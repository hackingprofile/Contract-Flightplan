//! The HTML report is a dashboard: a verdict badge at the top, summary cards,
//! a timeline, the contract delta as a table, before/after examples side by
//! side with each refusal's reason, and raw JSON folded away. It stays one
//! offline file with no scripts, fonts or network references, readable in
//! light and dark mode.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cfp-dashboard-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn cfp(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cfp"))
        .args(args)
        .output()
        .unwrap()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// Copy a pack, optionally flip the first `success` expectation in its catalog
/// so exactly one example fails, review and approve it, optionally verify, and
/// render the HTML report.
fn dashboard(name: &str, pack: &str, verify: bool, fail_one: bool) -> String {
    let dir = scratch(name);
    let src = root().join("packs").join(pack);
    let copy = |from: &Path, to: &Path| {
        fn walk(from: &Path, to: &Path) {
            fs::create_dir_all(to).unwrap();
            for e in fs::read_dir(from).unwrap() {
                let e = e.unwrap();
                let t = to.join(e.file_name());
                if e.file_type().unwrap().is_dir() {
                    walk(&e.path(), &t);
                } else {
                    fs::copy(e.path(), t).unwrap();
                }
            }
        }
        walk(from, to)
    };
    copy(&src, &dir);
    let (base, next, fixtures) = (
        dir.join("base.openapi.yaml"),
        dir.join("next.openapi.yaml"),
        dir.join("fixtures"),
    );
    if fail_one {
        let catalog = fixtures.join("catalog.json");
        let text = fs::read_to_string(&catalog).unwrap();
        fs::write(
            &catalog,
            text.replacen("\"expect\": \"success\"", "\"expect\": \"collision\"", 1),
        )
        .unwrap();
    }
    let record = dir.join("review.json");
    let proposal = dir.join("proposal.json");
    let r = cfp(&[
        "review",
        "--proposal",
        s(&proposal),
        "--source",
        s(&base),
        "--destination",
        s(&next),
        "--out",
        s(&record),
    ]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let migration = dir.join("migration.json");
    let a = cfp(&[
        "approve",
        "--proposal",
        s(&proposal),
        "--source",
        s(&base),
        "--destination",
        s(&next),
        "--fixtures",
        s(&fixtures),
        "--review",
        s(&record),
        "--reviewer",
        "local-reviewer",
        "--approved-at",
        "2026-10-30T09:00:00Z",
        "--out",
        s(&migration),
    ]);
    assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));
    let html = dir.join("r.html");
    let json = dir.join("r.json");
    let mut args = vec![
        "report",
        "--source",
        s(&base),
        "--destination",
        s(&next),
        "--proposal",
        s(&proposal),
        "--html",
        s(&html),
        "--json",
        s(&json),
    ];
    let verify_out = dir.join("verify.json");
    if verify {
        let v = cfp(&[
            "verify",
            "--migration",
            s(&migration),
            "--proposal",
            s(&proposal),
            "--source",
            s(&base),
            "--destination",
            s(&next),
            "--fixtures",
            s(&fixtures),
            "--out",
            s(&verify_out),
        ]);
        assert_eq!(
            v.status.code(),
            Some(if fail_one { 1 } else { 0 }),
            "{}",
            String::from_utf8_lossy(&v.stderr)
        );
        // The report re-runs verification itself; the verify file is checked.
        args.extend([
            "--migration",
            s(&migration),
            "--fixtures",
            s(&fixtures),
            "--verify",
            s(&verify_out),
        ]);
    }
    let out = cfp(&args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::read_to_string(html).unwrap()
}

fn assert_offline_and_themed(html: &str) {
    let lower = html.to_lowercase();
    assert!(!lower.contains("<script"), "no scripts");
    assert!(!lower.contains("<link"), "no linked files");
    assert!(
        !lower.contains("@import") && !lower.contains("url("),
        "no fetched CSS"
    );
    assert!(
        !lower.contains("http://") && !lower.contains("https://"),
        "no network references"
    );
    assert!(lower.contains("prefers-color-scheme: dark"), "dark mode");
    assert!(lower.contains("color-scheme: light dark"), "light and dark");
}

#[test]
fn a_stable_run_shows_a_green_badge_cards_timeline_and_delta_table() {
    let html = dashboard("stable", "rename-user-id", true, false);
    assert_offline_and_themed(&html);
    let badge = html
        .find("class=\"badge badge-stable\"")
        .expect("green stable badge");
    assert!(badge < html.find("Approval state").unwrap(), "badge first");
    for card in ["Approval", "Contracts", "Changes by class", "Examples"] {
        assert!(
            html.contains(&format!("<h3 class=\"card-title\">{card}</h3>")),
            "card {card}"
        );
    }
    let timeline = &html[html.find("id=\"timeline\"").expect("timeline")..];
    let timeline = &timeline[..timeline.find("</section>").unwrap()];
    for step in ["Compared", "Proposed", "Reviewed", "Approved", "Verified"] {
        assert!(timeline.contains(step), "timeline {step}: {timeline}");
    }
    assert!(timeline.contains("local-reviewer"), "{timeline}");
    assert!(html.contains("<table class=\"delta\">"), "delta table");
    assert!(html.contains("<th scope=\"col\">Field</th>"));
    assert!(
        html.contains("class=\"badge badge-breaking\""),
        "breaking badge"
    );
    assert!(html.contains("class=\"example\""), "example cards");
    assert!(html.contains("<details>"), "raw JSON folded");
    assert!(html.contains("7 of 7 passed"));
}

#[test]
fn a_failing_run_shows_a_red_badge_and_each_refusal_reason_inline() {
    let html = dashboard("failing", "safe-cast-count", true, true);
    assert_offline_and_themed(&html);
    assert!(html.contains("class=\"badge badge-unstable\""), "red badge");
    assert!(!html.contains("class=\"badge badge-stable\""));
    let examples = &html[html.find("id=\"examples\"").unwrap()..];
    let examples = &examples[..examples.find("</section>").unwrap()];
    // A refused example carries its reason next to it.
    assert!(examples.contains("class=\"reason\""), "{examples}");
    assert!(examples.contains("class=\"side before\""));
    assert!(examples.contains("class=\"side after\""));
}

/// Without a migration nothing is verified: the verdict is held, not guessed.
#[test]
fn a_run_without_verify_shows_an_amber_held_badge() {
    let html = dashboard("held", "rename-user-id", false, false);
    assert!(
        html.contains("class=\"badge badge-held\""),
        "amber held badge"
    );
    assert!(html.contains("Verdict: not run"));
}
