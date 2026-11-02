use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approve::{parse_migration_value, write_json, Migration};
use crate::diff::{compare_paths, DiffDocument};
use crate::error::{Error, Result};
use crate::verify::{review_proposal, verify_loaded_report, ReviewDocument, VerifyReport};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportDocument {
    pub schema_version: String,
    pub kind: String,
    pub approval_state: String,
    pub diff: DiffDocument,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewDocument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration: Option<Migration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyReport>,
}

/// Build the report. A verdict is never read from a file: with a migration,
/// the report re-runs verification itself from the reviewed proposal, the
/// migration and the fixtures folder, and the verdict, counts and failing list
/// come only from that fresh run. A verify file (`claimed`) is optional and
/// only checked: it is refused unless it matches the fresh run exactly.
pub fn build_report(
    source: &Path,
    destination: &Path,
    proposal: Option<&Path>,
    migration: Option<&Path>,
    fixtures: Option<&Path>,
    claimed: Option<&VerifyReport>,
) -> Result<ReportDocument> {
    let Some(migration_path) = migration else {
        if fixtures.is_some() || claimed.is_some() {
            return Err(Error::Invalid {
                message: "--fixtures and --verify need --migration and --proposal: a report's verdict comes only from re-running verification of an approved migration".into(),
            });
        }
        let diff = compare_paths(source, destination)?;
        let review = match proposal {
            Some(path) => Some(review_proposal(path, source, destination)?),
            None => None,
        };
        return Ok(ReportDocument {
            schema_version: "1".into(),
            kind: "report".into(),
            approval_state: "not_approved".into(),
            diff,
            review,
            migration: None,
            verify: None,
        });
    };
    let (Some(proposal_path), Some(fixtures_dir)) = (proposal, fixtures) else {
        return Err(Error::Invalid {
            message: "a report with --migration needs --proposal and --fixtures: it re-runs verification itself and takes the verdict only from that run, never from a verify file".into(),
        });
    };
    let diff = compare_paths(source, destination)?;
    let review = review_proposal(proposal_path, source, destination)?;
    // One read of the migration: the report shows the content it verified.
    let value = crate::proposal::load_json_value(migration_path)?;
    if value.get("kind").and_then(Value::as_str) == Some("proposal") {
        return Err(Error::Unapproved);
    }
    let mig = parse_migration_value(&value)?;
    let fresh = verify_loaded_report(
        migration_path,
        &value,
        &mig,
        proposal_path,
        source,
        destination,
        fixtures_dir,
    )?;
    if let Some(claimed) = claimed {
        check_claimed_verify(claimed, &fresh)?;
    }
    Ok(ReportDocument {
        schema_version: "1".into(),
        kind: "report".into(),
        approval_state: format!(
            "approved_by {} at {}",
            mig.approval.reviewer, mig.approval.approved_at
        ),
        diff,
        review: Some(review),
        migration: Some(mig),
        verify: Some(fresh),
    })
}

/// Refuse a verify file that differs in any way from the fresh run: its
/// migration id, approval, outcome count, any outcome's id, path, expect,
/// actual, flags or details, or the run's `stable`. Only `verified_at` (when
/// it ran) may differ.
pub fn check_claimed_verify(claimed: &VerifyReport, fresh: &VerifyReport) -> Result<()> {
    let differs = |what: String| {
        Error::Invalid {
        message: format!(
            "the verify result does not match a fresh verification run: {what}; the report takes its verdict only from the fresh run (rerun cfp verify, or leave out --verify)"
        ),
    }
    };
    for (what, a, b) in [
        (
            "schema_version",
            &claimed.schema_version,
            &fresh.schema_version,
        ),
        ("kind", &claimed.kind, &fresh.kind),
        ("migration id", &claimed.migration_id, &fresh.migration_id),
    ] {
        if a != b {
            return Err(differs(format!(
                "its {what} is {a:?}, the fresh run's is {b:?}"
            )));
        }
    }
    let (ca, fa) = (to_value(&claimed.approval)?, to_value(&fresh.approval)?);
    if ca != fa {
        let digests = [
            "source_contract",
            "destination_contract",
            "proposal",
            "rules",
            "review_record",
            "fixtures",
        ];
        let field = digests
            .iter()
            .find(|d| ca["bound_digests"][**d] != fa["bound_digests"][**d])
            .map(|d| format!("its {d} digest differs from the migration's"))
            .unwrap_or_else(|| "its approval record differs from the migration's".into());
        return Err(differs(field));
    }
    if claimed.outcomes.len() != fresh.outcomes.len() {
        return Err(differs(format!(
            "it lists {} outcome(s), the fresh run has {}",
            claimed.outcomes.len(),
            fresh.outcomes.len()
        )));
    }
    for (i, (c, f)) in claimed.outcomes.iter().zip(&fresh.outcomes).enumerate() {
        if c.id != f.id {
            return Err(differs(format!(
                "outcome {} is {:?}, the fresh run's is {:?}",
                i + 1,
                c.id,
                f.id
            )));
        }
        for (what, a, b) in [
            ("expect", &c.expect, &f.expect),
            ("actual", &c.actual, &f.actual),
            ("path", &c.path, &f.path),
        ] {
            if a != b {
                return Err(differs(format!(
                    "outcome {:?} has {what} {a:?}, the fresh run gives {b:?}",
                    c.id
                )));
            }
        }
        let (cv, fv) = (to_value(c)?, to_value(f)?);
        if cv != fv {
            let field = cv
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| k.clone())
                .chain(fv.as_object().into_iter().flatten().map(|(k, _)| k.clone()))
                .find(|k| cv.get(k) != fv.get(k))
                .unwrap_or_default();
            return Err(differs(format!("outcome {:?} differs in {field}", c.id)));
        }
    }
    if claimed.stable != fresh.stable {
        return Err(differs(format!(
            "it says stable {}, the fresh run says {}",
            claimed.stable, fresh.stable
        )));
    }
    Ok(())
}

/// A report part as a JSON value, for field-by-field comparison.
fn to_value<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| Error::Invalid {
        message: e.to_string(),
    })
}

pub fn write_report_pair(
    report: &ReportDocument,
    json_path: &Path,
    html_path: &Path,
) -> Result<()> {
    write_json(json_path, report)?;
    write_html(html_path, report)
}

pub fn write_markdown(path: &Path, report: &ReportDocument) -> Result<()> {
    crate::approve::write_output(path, render_markdown(report).as_bytes())
}

/// The run's verdict, from the report's own verification run alone.
struct Verdict<'a> {
    stable: bool,
    passed: usize,
    total: usize,
    failing: Vec<&'a str>,
}

fn verdict(report: &ReportDocument) -> Option<Verdict<'_>> {
    report.verify.as_ref().map(|v| Verdict {
        stable: v.stable,
        passed: v.outcomes.iter().filter(|o| o.passed).count(),
        total: v.outcomes.len(),
        failing: v
            .outcomes
            .iter()
            .filter(|o| !o.passed)
            .map(|o| o.id.as_str())
            .collect(),
    })
}

fn md_list(items: &[&str]) -> String {
    if items.is_empty() {
        "none".into()
    } else {
        items
            .iter()
            .map(|i| md_text(i))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn render_markdown(report: &ReportDocument) -> String {
    let mut out = String::from("# Contract Flightplan report\n\n## Verdict\n\n");
    match verdict(report) {
        Some(v) => out.push_str(&format!(
            "Stable: {}\n\nFixtures: {} of {} passed\n\nFailing fixtures: {}\n\n",
            if v.stable { "yes" } else { "no" },
            v.passed,
            v.total,
            md_list(&v.failing)
        )),
        None => out.push_str("Verdict: not run (no migration given, so nothing was verified)\n\n"),
    }
    out.push_str("## Approval\n\n");
    out.push_str(&format!(
        "Approval state: {}\n\n",
        md_text(&report.approval_state)
    ));
    out.push_str(&format!(
        "- Source: {} (`{}`)\n- Destination: {} (`{}`)\n\n",
        md_text(&report.diff.source.path),
        md_text(&report.diff.source.digest),
        md_text(&report.diff.destination.path),
        md_text(&report.diff.destination.digest)
    ));
    if let Some(review) = &report.review {
        out.push_str(&format!(
            "## Type-check\n\nState: {} ({} blocking)\n\n",
            md_text(&review.typecheck),
            review.blocking_diagnostics
        ));
        for d in &review.diagnostics {
            out.push_str(&format!(
                "- {:?} {} at {}: {}\n",
                d.severity,
                md_text(&d.rule_id),
                md_text(&d.location),
                md_text(&d.message)
            ));
        }
        if !review.diagnostics.is_empty() {
            out.push('\n');
        }
    }
    if let Some(verify) = &report.verify {
        out.push_str(&format!(
            "## Fixtures\n\nStable: {}\n\n",
            if verify.stable { "yes" } else { "no" }
        ));
        for item in &verify.outcomes {
            out.push_str(&format!(
                "- {} expect {} actual {} {}{}\n",
                md_text(&item.id),
                md_text(&item.expect),
                md_text(&item.actual),
                if item.passed { "pass" } else { "fail" },
                match (&item.message, item.passed) {
                    (Some(m), false) => format!(" — {}", md_text(m)),
                    _ => String::new(),
                }
            ));
        }
        out.push('\n');
    }
    out
}

pub fn render_job_summary(report: &ReportDocument) -> String {
    let mut out = String::from("## Contract Flightplan\n\n");
    match verdict(report) {
        Some(v) => out.push_str(&format!(
            "- stable: {}\n- fixtures: {}/{} passed\n- failing fixtures: {}\n",
            if v.stable { "yes" } else { "no" },
            v.passed,
            v.total,
            md_list(&v.failing)
        )),
        None => out.push_str("- stable: not run\n- fixtures: not run\n"),
    }
    out.push_str(&format!(
        "- approval: {}\n",
        md_text(&report.approval_state)
    ));
    if let Some(review) = &report.review {
        out.push_str(&format!(
            "- type-check: {} ({} blocking)\n",
            md_text(&review.typecheck),
            review.blocking_diagnostics
        ));
    }
    if let Some(verify) = &report.verify {
        let failures: Vec<&crate::verify::FixtureOutcome> =
            verify.outcomes.iter().filter(|o| !o.passed).collect();
        if !failures.is_empty() {
            out.push_str("\n### Failing fixtures\n\n");
            for f in failures {
                out.push_str(&format!(
                    "- {} expected {} got {}{}\n",
                    md_text(&f.id),
                    md_text(&f.expect),
                    md_text(&f.actual),
                    f.message
                        .as_deref()
                        .map(|m| format!(": {}", md_text(m)))
                        .unwrap_or_default()
                ));
            }
        }
    }
    out
}

pub fn write_html(path: &Path, report: &ReportDocument) -> Result<()> {
    crate::approve::write_output(path, render_html(report).as_bytes())
}

/// Styles of the offline dashboard: system fonts only, light by default and
/// dark under `prefers-color-scheme: dark`, nothing fetched.
const STYLE: &str = r#"
:root { color-scheme: light dark; --bg: #f5f6f8; --panel: #ffffff; --text: #1b2230; --muted: #5a6475; --border: #dde2ea; --accent: #2457d6; --code: #f0f2f6;
  --ok: #11683a; --ok-bg: #e2f5e9; --bad: #b42318; --bad-bg: #fde7e5; --warn: #8a5a00; --warn-bg: #fff2d6; --unk: #3f4a5e; --unk-bg: #e9edf3; }
@media (prefers-color-scheme: dark) { :root { --bg: #0e1117; --panel: #161b23; --text: #e6e9ef; --muted: #9aa4b4; --border: #2a313d; --accent: #82a7ff; --code: #10141b;
  --ok: #72dba0; --ok-bg: #11301f; --bad: #ff9488; --bad-bg: #3b1714; --warn: #f2c55c; --warn-bg: #39290b; --unk: #bcc5d4; --unk-bg: #242b37; } }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--text); font: 15px/1.5 system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
a { color: var(--accent); }
code, pre { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 13px; }
pre { white-space: pre-wrap; word-break: break-word; background: var(--code); border: 1px solid var(--border); padding: 10px 12px; border-radius: 8px; margin: 6px 0 0; }
.skip { position: absolute; left: -999px; }
.skip:focus { left: 16px; top: 8px; background: var(--panel); padding: 8px; z-index: 2; }
header.top { background: var(--panel); border-bottom: 1px solid var(--border); }
.wrap { max-width: 1080px; margin: 0 auto; padding: 0 16px; }
header.top .wrap { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: 8px; padding-top: 14px; padding-bottom: 10px; }
.brand { font-weight: 700; font-size: 18px; margin: 0; }
.tagline { color: var(--muted); margin: 0; font-size: 13px; }
nav ul { display: flex; flex-wrap: wrap; gap: 4px 16px; list-style: none; padding: 0; margin: 0 0 10px; font-size: 14px; }
main { padding-bottom: 40px; }
section { background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 16px 18px; margin: 16px 0; }
section > h2 { margin: 0 0 12px; font-size: 17px; }
h3 { font-size: 15px; margin: 14px 0 6px; }
h4 { font-size: 13px; margin: 0 0 4px; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
.muted { color: var(--muted); }
.badge { display: inline-block; border-radius: 999px; padding: 1px 10px; font-size: 12px; font-weight: 600; white-space: nowrap; border: 1px solid transparent; }
.badge-stable, .badge-pass, .badge-non-breaking { color: var(--ok); background: var(--ok-bg); }
.badge-unstable, .badge-fail, .badge-breaking { color: var(--bad); background: var(--bad-bg); }
.badge-held { color: var(--warn); background: var(--warn-bg); }
.badge-unknown, .badge-unchanged { color: var(--unk); background: var(--unk-bg); }
.verdict-hero { display: flex; flex-wrap: wrap; align-items: center; gap: 12px 16px; }
.verdict-hero .badge { font-size: 20px; padding: 6px 18px; }
.verdict-hero p { margin: 0; }
.cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; }
.card { border: 1px solid var(--border); border-radius: 10px; padding: 12px 14px; }
.card-title { margin: 0 0 6px; font-size: 13px; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
.card p { margin: 2px 0; }
.big { font-size: 22px; font-weight: 700; }
.timeline { list-style: none; margin: 0; padding: 0 0 0 4px; }
.timeline li { position: relative; padding: 0 0 12px 22px; border-left: 2px solid var(--border); margin-left: 6px; }
.timeline li:last-child { border-left-color: transparent; }
.timeline li::before { content: ""; position: absolute; left: -7px; top: 4px; width: 12px; height: 12px; border-radius: 50%; background: var(--accent); }
.timeline li.pending::before { background: var(--border); }
.timeline .when { color: var(--muted); font-size: 13px; }
.scroll { overflow-x: auto; }
table { width: 100%; border-collapse: collapse; font-size: 14px; }
th, td { border-bottom: 1px solid var(--border); text-align: left; padding: 6px 8px; vertical-align: top; }
th { color: var(--muted); font-weight: 600; font-size: 13px; }
.op { border-top: 1px solid var(--border); padding-top: 4px; margin-top: 10px; }
.op:first-of-type { border-top: 0; }
details { margin-top: 8px; }
summary { cursor: pointer; color: var(--accent); font-size: 14px; }
.example { border: 1px solid var(--border); border-radius: 10px; padding: 10px 12px; margin: 10px 0; }
.example header { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
.example h3 { margin: 0; }
.example .expect { margin: 4px 0; color: var(--muted); font-size: 14px; }
.reason { margin: 6px 0; padding: 6px 10px; border-left: 3px solid var(--warn); background: var(--warn-bg); border-radius: 4px; }
.sides { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; margin-top: 6px; }
.side { min-width: 0; }
tr.bad td:first-child { box-shadow: inset 3px 0 0 var(--bad); }
@media (max-width: 640px) { .sides { grid-template-columns: 1fr; } section { padding: 12px; } }
"#;

/// Badge class and label for a compatibility class.
fn compat_badge(c: crate::diff::Compatibility) -> String {
    use crate::diff::Compatibility::*;
    let (class, label) = match c {
        Unchanged => ("unchanged", "unchanged"),
        NonBreaking => ("non-breaking", "non-breaking"),
        Unknown => ("unknown", "unknown"),
        Breaking => ("breaking", "breaking"),
    };
    format!("<span class=\"badge badge-{class}\">{label}</span>")
}

fn compat_name(c: crate::diff::Compatibility) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn pass_badge(passed: bool) -> &'static str {
    if passed {
        "<span class=\"badge badge-pass\">passed</span>"
    } else {
        "<span class=\"badge badge-fail\">failed</span>"
    }
}

pub fn render_html(report: &ReportDocument) -> String {
    let sections: [(&str, &str, String); 9] = [
        ("verdict", "Verdict", verdict_html(report)),
        ("summary", "Summary", summary_html(report)),
        ("timeline", "Timeline", timeline_html(report)),
        ("delta", "Contract delta", diff_html(&report.diff)),
        (
            "examples",
            "Before / after examples",
            examples_html(report.verify.as_ref()),
        ),
        ("rules", "Rules", rules_section_html(report)),
        (
            "uncertainty",
            "Diagnostics and uncertainty",
            uncertainty_html(report),
        ),
        (
            "approval",
            "Approval",
            approval_html(report.migration.as_ref(), &report.approval_state),
        ),
        (
            "fixtures",
            "Fixture outcomes",
            fixtures_html(report.verify.as_ref()),
        ),
    ];
    let nav: String = sections
        .iter()
        .map(|(id, title, _)| format!("<li><a href=\"#{id}\">{title}</a></li>"))
        .collect();
    let body: String = sections
        .iter()
        .map(|(id, title, inner)| section(id, title, inner))
        .collect();
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src 'none'; base-uri 'none'; form-action 'none'">
<title>Contract Flightplan review</title>
<style>{STYLE}</style>
</head>
<body>
<a class="skip" href="#main">Skip to content</a>
<header class="top">
<div class="wrap">
<div><h1 class="brand">Contract Flightplan review</h1>
<p class="tagline">Offline static report. Untrusted fixture text is escaped and is not executed.</p></div>
</div>
<nav class="wrap" aria-label="Report sections"><ul>{nav}</ul></nav>
</header>
<main id="main" class="wrap">
{body}
</main>
</body>
</html>
"##
    )
}

fn verdict_html(report: &ReportDocument) -> String {
    match verdict(report) {
        Some(v) => {
            let failing = if v.failing.is_empty() {
                "none".to_string()
            } else {
                v.failing
                    .iter()
                    .map(|id| escape_inline(id))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let (class, label) = if v.stable {
                ("stable", "Stable")
            } else {
                ("unstable", "Not stable")
            };
            format!(
                "<div class=\"verdict-hero\"><span class=\"badge badge-{class}\">{label}</span><div><p class=\"{}\">Verdict: <strong>{}</strong>: {} of {} passed.</p><p class=\"muted\">Failing fixtures: {failing}</p></div></div>",
                if v.stable { "ok" } else { "bad" },
                if v.stable { "stable" } else { "not stable" },
                v.passed,
                v.total
            )
        }
        None => "<div class=\"verdict-hero\"><span class=\"badge badge-held\">Held</span><div><p>Verdict: not run. No migration was given, so nothing was verified; pass <code>--migration</code>, <code>--proposal</code> and <code>--fixtures</code> to verify it.</p></div></div>".into(),
    }
}

fn section(id: &str, title: &str, inner: &str) -> String {
    format!("<section id=\"{id}\" aria-labelledby=\"{id}-h\"><h2 id=\"{id}-h\">{title}</h2>{inner}</section>")
}

fn card(title: &str, inner: &str) -> String {
    format!("<div class=\"card\"><h3 class=\"card-title\">{title}</h3>{inner}</div>")
}

/// Short form of a `sha256:<hex>` digest for display; the full value is in the title.
fn short_digest(digest: &str) -> String {
    let short: String = digest.chars().take(19).collect();
    format!(
        "<code title=\"{}\">{}</code>",
        escape_inline(digest),
        escape_inline(&short)
    )
}

fn summary_html(report: &ReportDocument) -> String {
    let approval = match &report.migration {
        Some(m) => format!(
            "<p>Approval state: <strong>{}</strong></p><p class=\"muted\">Reviewer: {}<br>Approved at: {}</p>",
            escape_inline(&report.approval_state),
            escape_inline(&m.approval.reviewer),
            escape_inline(&m.approval.approved_at)
        ),
        None => format!(
            "<p>Approval state: <strong>{}</strong></p><p class=\"muted\">No approved migration was supplied.</p>",
            escape_inline(&report.approval_state)
        ),
    };
    let d = &report.diff;
    let contracts = format!(
        "<p><strong>{}</strong> → <strong>{}</strong></p><p class=\"muted\">OpenAPI {} → {}</p><p class=\"muted\">{} → {}</p>",
        escape_inline(&d.source.path),
        escape_inline(&d.destination.path),
        escape_inline(&d.source.openapi),
        escape_inline(&d.destination.openapi),
        short_digest(&d.source.digest),
        short_digest(&d.destination.digest)
    );
    let mut counts = std::collections::BTreeMap::new();
    for op in &d.operations {
        let changes = op
            .request
            .iter()
            .chain(op.responses.values())
            .flat_map(|b| b.changes.iter().map(|c| c.compatibility))
            .chain(op.parameter_changes.iter().map(|p| p.compatibility));
        for c in changes {
            *counts.entry(c).or_insert(0usize) += 1;
        }
    }
    let count = |c| counts.get(&c).copied().unwrap_or(0);
    use crate::diff::Compatibility::*;
    let changes = format!(
        "<p>{} <span class=\"big\">{}</span></p><p>{} {} &nbsp; {} {}</p><p class=\"muted\">Operations: {} compared, {} added, {} removed</p>",
        compat_badge(Breaking),
        count(Breaking),
        compat_badge(NonBreaking),
        count(NonBreaking),
        compat_badge(Unknown),
        count(Unknown),
        d.operations.len(),
        d.added_operations.len(),
        d.removed_operations.len()
    );
    let examples = match &report.verify {
        Some(v) => {
            let passed = v.outcomes.iter().filter(|o| o.passed).count();
            format!(
                "<p><span class=\"big\">{passed}</span> passed</p><p><span class=\"big\">{}</span> failed</p>",
                v.outcomes.len() - passed
            )
        }
        None => "<p class=\"muted\">Not verified yet.</p>".into(),
    };
    format!(
        "<div class=\"cards\">{}{}{}{}</div>",
        card("Approval", &approval),
        card("Contracts", &contracts),
        card("Changes by class", &changes),
        card("Examples", &examples)
    )
}

fn timeline_html(report: &ReportDocument) -> String {
    let item = |done: bool, what: &str, detail: String, when: &str| {
        format!(
            "<li class=\"{}\"><strong>{what}</strong> <span class=\"when\">{}</span><br>{detail}</li>",
            if done { "done" } else { "pending" },
            escape_inline(when)
        )
    };
    let d = &report.diff;
    let mut s = String::from("<ol class=\"timeline\">");
    s.push_str(&item(
        true,
        "Compared",
        format!(
            "{} → {}",
            escape_inline(&d.source.path),
            escape_inline(&d.destination.path)
        ),
        "",
    ));
    let proposal_id = report
        .review
        .as_ref()
        .map(|r| r.proposal_id.clone())
        .or_else(|| report.migration.as_ref().map(|m| m.id.clone()));
    match proposal_id {
        Some(id) => {
            let by = if id.starts_with("example-local") {
                "suggested by the built-in proposer"
            } else {
                "from a proposal file"
            };
            s.push_str(&item(
                true,
                "Proposed",
                format!("Proposal {} ({by})", escape_inline(&id)),
                "",
            ));
        }
        None => s.push_str(&item(false, "Proposed", "No proposal supplied.".into(), "")),
    }
    let history = report.migration.as_ref().map(|m| &m.history);
    match history.and_then(|h| h.iter().find(|e| e.action == "reviewed")) {
        Some(e) => s.push_str(&item(
            true,
            "Reviewed",
            format!("by {}", escape_inline(&e.actor)),
            &e.at,
        )),
        None => match &report.review {
            Some(r) => s.push_str(&item(
                true,
                "Reviewed",
                format!("Type-check {}", escape_inline(&r.typecheck)),
                "",
            )),
            None => s.push_str(&item(false, "Reviewed", "Not reviewed.".into(), "")),
        },
    }
    match &report.migration {
        Some(m) => s.push_str(&item(
            true,
            "Approved",
            format!("by {}", escape_inline(&m.approval.reviewer)),
            &m.approval.approved_at,
        )),
        None => s.push_str(&item(false, "Approved", "Not approved.".into(), "")),
    }
    match &report.verify {
        Some(v) => {
            let passed = v.outcomes.iter().filter(|o| o.passed).count();
            let when = v.verified_at.as_deref().unwrap_or("time not recorded");
            s.push_str(&item(
                true,
                "Verified",
                format!("{passed} of {} examples passed", v.outcomes.len()),
                when,
            ));
        }
        None => s.push_str(&item(false, "Verified", "Not verified.".into(), "")),
    }
    s.push_str("</ol>");
    s
}

fn diff_html(diff: &DiffDocument) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "<p>Matched operations: {}.</p>",
        diff.operations.len()
    ));
    if !diff.added_operations.is_empty() {
        s.push_str(&format!(
            "<p>Added: {}</p>",
            escape(&format!("{:?}", diff.added_operations))
        ));
    }
    if !diff.removed_operations.is_empty() {
        s.push_str(&format!(
            "<p>Removed: {}</p>",
            escape(&format!("{:?}", diff.removed_operations))
        ));
    }
    let op_name =
        |op: &crate::parse::OpIdentity| escape_inline(&format!("{} {}", op.method, op.path));
    for moved in &diff.moved_operations {
        s.push_str(&format!(
            "<p>Moved: {} → {}</p>",
            op_name(&moved.from),
            op_name(&moved.to)
        ));
    }
    for affected in &diff.affected_schemas {
        let ops: Vec<String> = affected.operations.iter().map(op_name).collect();
        s.push_str(&format!(
            "<p>Affected component {}: {}</p>",
            escape_inline(&affected.component_pointer),
            ops.join(", ")
        ));
    }
    for op in &diff.operations {
        s.push_str(&format!(
            "<div class=\"op\"><h3>{} {} {}</h3>",
            escape(&op.identity.method),
            escape(&op.identity.path),
            compat_badge(op.compatibility)
        ));
        if let Some(req) = &op.request {
            s.push_str(&format!(
                "<p class=\"muted\">Request compatibility: {}. Request added {}, removed {}.</p>",
                compat_name(req.compatibility),
                list_or_none(&req.added_properties),
                list_or_none(&req.removed_properties)
            ));
        }
        for (status, body) in &op.responses {
            s.push_str(&format!(
                "<p class=\"muted\">Response {} compatibility: {}.</p>",
                escape_inline(status),
                compat_name(body.compatibility)
            ));
        }
        let mut rows = String::new();
        let bodies = op.request.iter().map(|b| ("request".to_string(), b)).chain(
            op.responses
                .iter()
                .map(|(status, b)| (format!("response {status}"), b)),
        );
        for (place, body) in bodies {
            for c in &body.changes {
                let field = if c.field.is_empty() {
                    "(body)".to_string()
                } else {
                    c.field.clone()
                };
                rows.push_str(&format!(
                    "<tr><td><code>{}</code> <span class=\"muted\">{}</span></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape_inline(&field),
                    escape_inline(&place),
                    escape_inline(&compat_name_kind(&c.kind)),
                    compat_badge(c.compatibility),
                    escape_inline(&c.reason)
                ));
            }
        }
        for p in &op.parameter_changes {
            rows.push_str(&format!(
                "<tr><td><code>{}</code> <span class=\"muted\">{} parameter</span></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_inline(&p.name),
                escape_inline(&p.location),
                escape_inline(&p.kind.replace('_', " ")),
                compat_badge(p.compatibility),
                escape_inline(&p.reason)
            ));
        }
        if rows.is_empty() {
            s.push_str("<p>No body or parameter changes.</p>");
        } else {
            s.push_str(&format!(
                "<div class=\"scroll\"><table class=\"delta\"><thead><tr><th scope=\"col\">Field</th><th scope=\"col\">Change</th><th scope=\"col\">Class</th><th scope=\"col\">Reason</th></tr></thead><tbody>{rows}</tbody></table></div>"
            ));
        }
        s.push_str(&format!(
            "<details><summary>Raw JSON for this operation</summary><pre>{}</pre></details></div>",
            escape(&pretty(op))
        ));
    }
    if !diff.unsupported.is_empty() {
        s.push_str(&format!(
            "<h3>Unsupported</h3><details><summary>Raw JSON for {} unsupported construct(s)</summary><pre>{}</pre></details>",
            diff.unsupported.len(),
            escape(&pretty(&diff.unsupported))
        ));
    }
    s
}

/// Escaped, comma-joined names, or "none".
fn list_or_none(names: &[String]) -> String {
    if names.is_empty() {
        "none".into()
    } else {
        escape(&names.join(", "))
    }
}

/// A change kind in words: `property_added` reads "property added".
fn compat_name_kind(kind: &crate::diff::ChangeKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.replace('_', " ")))
        .unwrap_or_default()
}

fn rules_section_html(report: &ReportDocument) -> String {
    if let Some(review) = &report.review {
        rules_html(review)
    } else if let Some(migration) = &report.migration {
        migration_rules_html(migration)
    } else {
        "<p>No proposal or migration supplied; rules are shown when <code>--proposal</code> or <code>--migration</code> is given.</p>".into()
    }
}

fn rules_html(review: &ReviewDocument) -> String {
    let mut s = String::new();
    for rule in &review.rules {
        s.push_str(&format!(
            "<h3>{} <span class=\"muted\">({})</span></h3><p>{}</p><p class=\"muted\">Lossiness: {}. Reversibility: {}.</p>",
            escape(&rule.id),
            escape(&rule.operation),
            escape(&rule.summary),
            escape(&rule.lossiness),
            escape(&rule.reversibility)
        ));
    }
    s
}

fn uncertainty_html(report: &ReportDocument) -> String {
    let mut s = String::new();
    match &report.review {
        Some(review) => {
            s.push_str(&format!(
                "<p>Type-check: <strong>{}</strong> ({} blocking diagnostic(s)).</p>",
                escape(&review.typecheck),
                review.blocking_diagnostics
            ));
            if review.diagnostics.is_empty() {
                s.push_str("<p>No type-check diagnostics.</p>");
            } else {
                s.push_str("<table><thead><tr><th>Severity</th><th>Rule</th><th>Location</th><th>Message</th></tr></thead><tbody>");
                for d in &review.diagnostics {
                    let class = if d.is_error() { "bad" } else { "" };
                    s.push_str(&format!(
                        "<tr class=\"{class}\"><td>{:?}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                        d.severity,
                        escape_inline(&d.rule_id),
                        escape_inline(&d.location),
                        escape_inline(&d.message)
                    ));
                }
                s.push_str("</tbody></table>");
            }
            if !review.alternatives.is_empty() {
                s.push_str("<h3>Alternatives</h3><ul>");
                for alt in &review.alternatives {
                    s.push_str(&format!(
                        "<li>{} {}: {} ({})</li>",
                        escape_inline(&alt.rule_id),
                        escape_inline(&alt.operation),
                        escape_inline(&alt.parameters.to_string()),
                        escape_inline(&alt.why)
                    ));
                }
                s.push_str("</ul>");
            }
            if !review.assumptions.is_empty() {
                s.push_str("<h3>Assumptions</h3><ul>");
                for assumption in &review.assumptions {
                    s.push_str(&format!("<li>{}</li>", escape_inline(assumption)));
                }
                s.push_str("</ul>");
            }
            if review.ambiguity.is_empty() {
                s.push_str("<p>No ambiguity notes.</p>");
            } else {
                s.push_str("<h3>Ambiguity</h3><ul>");
                for note in &review.ambiguity {
                    s.push_str(&format!("<li>{}</li>", escape(note)));
                }
                s.push_str("</ul>");
            }
        }
        None => s.push_str("<p>No proposal supplied; type-check diagnostics are shown when <code>--proposal</code> is given.</p>"),
    }
    s.push_str(&format!(
        "<p>Unsupported constructs: {}. Unknown constructs: {}.</p>",
        report.diff.unsupported.len(),
        report.diff.unknown.len()
    ));
    let notes: Vec<_> = report
        .diff
        .unsupported
        .iter()
        .chain(report.diff.unknown.iter())
        .collect();
    if !notes.is_empty() {
        s.push_str("<ul>");
        for note in notes {
            s.push_str(&format!(
                "<li><code>{}</code> at <code>{}</code>: {}</li>",
                escape_inline(&note.construct),
                escape_inline(&note.location),
                escape_inline(&note.message)
            ));
        }
        s.push_str("</ul>");
    }
    if let Some(verify) = &report.verify {
        let mut events: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for outcome in &verify.outcomes {
            for event in &outcome.lossiness {
                *events.entry(event.as_str()).or_insert(0) += 1;
            }
        }
        if events.is_empty() {
            s.push_str("<p>No lossiness events were observed on the fixtures.</p>");
        } else {
            s.push_str("<h3>Observed lossiness</h3><ul>");
            for (event, count) in events {
                s.push_str(&format!(
                    "<li>{} on {count} fixture outcome(s)</li>",
                    escape(event)
                ));
            }
            s.push_str("</ul>");
        }
    }
    s
}

fn migration_rules_html(migration: &Migration) -> String {
    let mut s = String::new();
    for rule in &migration.rules {
        s.push_str(&format!(
            "<h3>{} <span class=\"muted\">({})</span></h3><p>{}</p><p class=\"muted\">Lossiness: {}. Reversibility: {}.</p><details><summary>Raw JSON for this rule</summary><pre>{}</pre></details>",
            escape(&rule.id),
            escape(&rule.operation),
            escape(&rule.evidence),
            escape(&rule.lossiness),
            escape(&rule.reversibility),
            escape(&pretty(rule))
        ));
    }
    s
}

fn examples_html(verify: Option<&VerifyReport>) -> String {
    let Some(verify) = verify else {
        return "<p>Not verified: no migration was given.</p>".into();
    };
    let mut s = String::new();
    for item in &verify.outcomes {
        let reason = match (&item.message, item.passed) {
            (Some(message), _) => format!("<p class=\"reason\">{}</p>", escape_inline(message)),
            (None, false) => format!(
                "<p class=\"reason\">Expected the outcome {}, but the run ended in {}.</p>",
                escape_inline(&item.expect),
                escape_inline(&item.actual)
            ),
            (None, true) => String::new(),
        };
        let after = match &item.after {
            Some(after) => format!("<pre>{}</pre>", escape(&pretty(after))),
            None => format!(
                "<p class=\"muted\">No output: the example was refused ({}).</p>",
                escape_inline(&item.actual)
            ),
        };
        s.push_str(&format!(
            "<article class=\"example\"><header><h3>{}</h3>{}</header><p class=\"expect\">Expected {}, got {}.</p>{reason}<div class=\"sides\"><div class=\"side before\"><h4>Before</h4><pre>{}</pre></div><div class=\"side after\"><h4>After</h4>{after}</div></div></article>",
            escape(&item.id),
            pass_badge(item.passed),
            escape_inline(&item.expect),
            escape_inline(&item.actual),
            escape(&pretty(&item.before))
        ));
    }
    s
}

fn approval_html(migration: Option<&Migration>, state: &str) -> String {
    match migration {
        Some(m) => format!(
            "<p>{}</p><p class=\"muted\">{}</p><details><summary>Raw approval JSON</summary><pre>{}</pre></details>",
            escape_inline(state),
            escape(&m.approval.identity_boundary),
            escape(&pretty(&m.approval))
        ),
        None => format!("<p>{}</p>", escape_inline(state)),
    }
}

fn fixtures_html(verify: Option<&VerifyReport>) -> String {
    let Some(verify) = verify else {
        return "<p>No fixture outcomes.</p>".into();
    };
    let mut rows = String::new();
    for item in &verify.outcomes {
        let class = if item.passed { "ok" } else { "bad" };
        rows.push_str(&format!(
            "<tr class=\"{class}\"><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape_inline(&item.id),
            escape_inline(&item.expect),
            escape_inline(&item.actual),
            pass_badge(item.passed),
            escape_inline(item.message.as_deref().unwrap_or(""))
        ));
    }
    format!("<div class=\"scroll\"><table><thead><tr><th scope=\"col\">Id</th><th scope=\"col\">Expect</th><th scope=\"col\">Actual</th><th scope=\"col\">Result</th><th scope=\"col\">Message</th></tr></thead><tbody>{rows}</tbody></table></div><details><summary>Raw verify JSON</summary><pre>{}</pre></details>", escape(&pretty(verify)))
}

fn pretty<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into())
}

/// Characters that would hide or reorder text when shown: control
/// characters, Unicode line separators and bidirectional controls.
pub fn is_hidden(ch: char) -> bool {
    // Format characters (Unicode Cf: zero-width spaces and joiners, bidi
    // marks, embeddings and isolates, U+061C, U+FEFF, the soft hyphen, tags),
    // variation selectors, U+034F and the Hangul fillers draw nothing or
    // reorder text, so two different names could look identical.
    static INVISIBLE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let invisible = INVISIBLE.get_or_init(|| {
        regex::Regex::new(
            r"^[\p{Cf}\p{Variation_Selector}\u{034F}\u{115F}\u{1160}\u{3164}\u{FFA0}]$",
        )
        .expect("a fixed, valid character class")
    });
    ch.is_control()
        || matches!(ch, '\u{2028}' | '\u{2029}')
        || (!ch.is_ascii() && invisible.is_match(ch.encode_utf8(&mut [0; 4])))
}

/// A visible stand-in for a hidden character: `\n`, `\r`, `\t` or `\u{..}`.
fn visible(ch: char) -> String {
    match ch {
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        other => format!("\\u{{{:04x}}}", other as u32),
    }
}

/// Untrusted text for one terminal line (an error on stderr, a guide line):
/// every hidden character, line breaks included, is shown as an escape, so no
/// control sequence reaches the terminal and nothing can start a new line;
/// and a leading `::` (a GitHub Actions workflow command) is broken up.
pub fn terminal_line(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if is_hidden(ch) {
            out.push_str(&visible(ch));
        } else {
            out.push(ch);
        }
    }
    neutralize_workflow_command(out)
}

/// Like `terminal_line` for text that is several lines on purpose (a usage
/// message): line breaks are kept, and each line gets the same treatment.
pub fn terminal_text(input: &str) -> String {
    input
        .split('\n')
        .map(|line| terminal_line(line.strip_suffix('\r').unwrap_or(line)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn neutralize_workflow_command(mut line: String) -> String {
    let start = line.len() - line.trim_start().len();
    if line[start..].starts_with("::") {
        line.replace_range(start..start + 1, "\\u{003a}");
    }
    line
}

/// Escape text for HTML. Newlines and tabs are kept (for `<pre>` blocks);
/// every other hidden character is shown as an escape.
pub fn escape(input: &str) -> String {
    escape_with(input, |ch| {
        is_hidden(ch) && !matches!(ch, '\n' | '\r' | '\t')
    })
}

/// Escape untrusted text for one line of HTML: like `escape`, and newlines
/// and tabs are shown as escapes too, so a value cannot start a new line.
pub fn escape_inline(input: &str) -> String {
    escape_with(input, is_hidden)
}

fn escape_with(input: &str, hide: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if hide(c) => out.push_str(&visible(c)),
            _ => out.push(ch),
        }
    }
    out
}

/// Neutralize untrusted text for one line of Markdown (reports and the CI job
/// summary): hidden characters become visible escapes, so the text cannot
/// start a line, list item or heading; `<`, `>` and `&` become entities, so
/// it cannot open HTML or a comment; and inline Markdown punctuation is
/// backslash-escaped, so it cannot emphasize, link or open a code span.
pub fn md_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\\' | '`' | '*' | '_' | '[' | ']' | '|' | '~' | '!' | '#' => {
                out.push('\\');
                out.push(ch);
            }
            c if is_hidden(c) => out.push_str(&visible(c)),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_text_is_escaped() {
        let escaped = escape("<script>alert(1)</script>");
        assert!(!escaped.contains("<script>"));
        assert!(escaped.contains("&lt;script&gt;"));
    }

    #[test]
    fn terminal_text_escapes_controls_and_breaks_up_workflow_commands() {
        assert_eq!(
            terminal_line("a\u{1b}[2J\n::error::x"),
            "a\\u{001b}[2J\\n::error::x"
        );
        assert_eq!(
            terminal_line("::add-mask::stable"),
            "\\u{003a}:add-mask::stable"
        );
        assert_eq!(terminal_line("  ::warning::x"), "  \\u{003a}:warning::x");
        assert_eq!(
            terminal_text("usage\r\n::error::x\n\u{9b}31m"),
            "usage\n\\u{003a}:error::x\n\\u{009b}31m"
        );
        assert_eq!(terminal_line("plain text: fine"), "plain text: fine");
    }
}
