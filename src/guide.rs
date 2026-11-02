//! `cfp guide`: one person walks through compare, propose, review, approve,
//! verify and report in the terminal. Each step says in plain words what it
//! does and what it found, the proposal says who suggested it, and nothing is
//! approved until the person answers `y` to `Approve this mapping? [y/N]` (or
//! passes `--yes`). Every step uses the same library calls as the separate
//! commands, so the files it writes are the same files they write.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::approve::{approve, write_json};
use crate::diff::{compare_paths, Compatibility, DiffDocument};
use crate::error::{Error, Result};
use crate::provider::{run_provider, ExampleLocalProvider, ProposalRequest};
use crate::report::{build_report, terminal_line, write_markdown, write_report_pair};
use crate::verify::{first_failure, review_proposal, verify_paths_report};

/// What the guide was given on the command line.
pub struct GuideOptions {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub fixtures: PathBuf,
    pub out_dir: PathBuf,
    /// A person's proposal file; without it the built-in proposer suggests one.
    pub proposal: Option<PathBuf>,
    /// Recorded as the reviewer. Asked for when absent.
    pub reviewer: Option<String>,
    pub note: Option<String>,
    pub approved_at: Option<String>,
    /// Approve without asking (the answer `y`, given up front).
    pub yes: bool,
}

const STEPS: usize = 6;

fn io_err(e: std::io::Error) -> Error {
    Error::io("terminal", e)
}

fn step(out: &mut dyn Write, n: usize, title: &str, explain: &str) -> Result<()> {
    writeln!(out, "\nStep {n} of {STEPS}: {title}\n{explain}").map_err(io_err)
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn class(c: Compatibility) -> &'static str {
    match c {
        Compatibility::Unchanged => "unchanged",
        Compatibility::NonBreaking => "non-breaking",
        Compatibility::Unknown => "unknown",
        Compatibility::Breaking => "breaking",
    }
}

/// Read one answer line; `None` at end of input.
fn read_line(input: &mut dyn BufRead) -> Result<Option<String>> {
    let mut line = String::new();
    let n = input.read_line(&mut line).map_err(io_err)?;
    Ok((n > 0).then(|| line.trim().to_string()))
}

/// Ask `Approve this mapping? [y/N]`. Only `y` or `yes` (any case) approves;
/// an empty answer, any other answer or the end of input is No.
pub fn ask_approval(input: &mut dyn BufRead, out: &mut dyn Write) -> Result<bool> {
    write!(out, "Approve this mapping? [y/N] ").map_err(io_err)?;
    out.flush().map_err(io_err)?;
    let answer = read_line(input)?;
    if answer.is_none() {
        writeln!(out).map_err(io_err)?;
    }
    Ok(answer.is_some_and(|a| a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes")))
}

fn summarize_diff(out: &mut dyn Write, diff: &DiffDocument) -> Result<()> {
    let w = |out: &mut dyn Write, s: String| writeln!(out, "{}", terminal_line(&s)).map_err(io_err);
    w(
        out,
        format!(
            "Source: {} (OpenAPI {}). Destination: {} (OpenAPI {}).",
            diff.source.path, diff.source.openapi, diff.destination.path, diff.destination.openapi
        ),
    )?;
    w(
        out,
        format!(
            "{} operation(s) in both, {} added, {} removed, {} moved.",
            diff.operations.len(),
            diff.added_operations.len(),
            diff.removed_operations.len(),
            diff.moved_operations.len()
        ),
    )?;
    let mut shown = 0;
    for op in &diff.operations {
        w(
            out,
            format!(
                "  {} {}: {}",
                op.identity.method,
                op.identity.path,
                class(op.compatibility)
            ),
        )?;
        let bodies = op.request.iter().map(|b| ("request".to_string(), b)).chain(
            op.responses
                .iter()
                .map(|(s, b)| (format!("response {s}"), b)),
        );
        for (place, body) in bodies {
            for change in &body.changes {
                shown += 1;
                let field = if change.field.is_empty() {
                    "(body)"
                } else {
                    &change.field
                };
                w(
                    out,
                    format!(
                        "    {place} {field}: {} - {}",
                        class(change.compatibility),
                        change.reason
                    ),
                )?;
            }
        }
        for p in &op.parameter_changes {
            shown += 1;
            w(
                out,
                format!(
                    "    {} parameter {}: {} - {}",
                    p.location,
                    p.name,
                    class(p.compatibility),
                    p.reason
                ),
            )?;
        }
    }
    if shown == 0 {
        w(out, "  No body or parameter changes.".into())?;
    }
    if !diff.unsupported.is_empty() {
        w(
            out,
            format!(
                "  {} construct(s) are outside the supported subset and stay visible as unsupported.",
                diff.unsupported.len()
            ),
        )?;
    }
    Ok(())
}

/// Run the whole guided flow. Returns `Ok(())` when the mapping was approved
/// and every example passed; a refusal, a decline or a failing example is an
/// error with the same exit code the separate command would give.
pub fn run_guide(
    options: &GuideOptions,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<()> {
    let dir = &options.out_dir;
    // No file the guide writes may replace one of its inputs; checked before
    // the output folder is made, so a refused run creates nothing.
    let written: Vec<PathBuf> = [
        "diff.json",
        "proposal.json",
        "review.json",
        "migration.json",
        "verify.json",
        "report.html",
        "report.json",
        "report.md",
    ]
    .iter()
    .map(|name| dir.join(name))
    .collect();
    // The fixtures folder is an input too: nothing may be written inside it.
    let mut inputs = vec![
        options.source.as_path(),
        options.destination.as_path(),
        options.fixtures.as_path(),
    ];
    inputs.extend(options.proposal.as_deref());
    let outputs: Vec<&Path> = written.iter().map(PathBuf::as_path).collect();
    crate::approve::check_outputs(&outputs, &inputs)?;
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.display().to_string(), e))?;
    let say =
        |out: &mut dyn Write, s: String| writeln!(out, "{}", terminal_line(&s)).map_err(io_err);
    say(
        out,
        format!(
            "Contract Flightplan guide. Files are written to {}.",
            dir.display()
        ),
    )?;

    // 1. Compare.
    step(
        out,
        1,
        "Compare the two contracts",
        "Every operation in both contracts is matched and its request, responses and parameters are compared. Each change gets a class: breaking, non-breaking or unknown.",
    )?;
    let diff = compare_paths(&options.source, &options.destination)?;
    write_json(&dir.join("diff.json"), &diff)?;
    summarize_diff(out, &diff)?;

    // 2. Propose.
    step(
        out,
        2,
        "Suggest a mapping",
        "A mapping is a list of rules that turn old payloads into new ones. A suggestion is never executable by itself: only your approval in step 4 makes it a migration.",
    )?;
    let proposal_path = match &options.proposal {
        Some(path) => {
            say(
                out,
                format!(
                    "Suggested by: a person, in the proposal file {}.",
                    name_of(path)
                ),
            )?;
            path.clone()
        }
        None => {
            let request = ProposalRequest::new(&options.source, &options.destination, &[])?;
            let provider = ExampleLocalProvider {
                source: options.source.clone(),
                destination: options.destination.clone(),
            };
            let accepted = run_provider(&provider, &request)?;
            let path = dir.join("proposal.json");
            write_json(&path, &accepted.proposal)?;
            say(out, "Suggested by: the built-in proposer (local rules of thumb, no model, nothing sent anywhere). Check its guesses:".into())?;
            for assumption in &accepted.proposal.assumptions {
                say(out, format!("  - {assumption}"))?;
            }
            say(
                out,
                format!("The suggestion was written to {}.", path.display()),
            )?;
            path
        }
    };
    let proposal = crate::proposal::load_proposal(&proposal_path)?;
    if proposal.rules.is_empty() {
        say(out, "The suggestion has no rules.".into())?;
    }
    for (i, rule) in proposal.rules.iter().enumerate() {
        say(
            out,
            format!(
                "  {}. {} ({}) on {} {} {}: {}; lossiness {}, {}",
                i + 1,
                rule.id,
                rule.operation,
                rule.source.operation.method,
                rule.source.operation.path,
                rule.source.location,
                rule.parameters,
                rule.lossiness,
                rule.reversibility
            ),
        )?;
    }
    for alt in &proposal.alternatives {
        say(
            out,
            format!(
                "  Alternative for {}: {} {} ({})",
                alt.rule_id, alt.operation, alt.parameters, alt.why
            ),
        )?;
    }

    // 3. Review.
    step(
        out,
        3,
        "Review and type-check",
        "Each rule is explained and checked against both contracts: field types, enum values and required fields must line up before anything can be approved.",
    )?;
    let review = review_proposal(&proposal_path, &options.source, &options.destination)?;
    let review_path = dir.join("review.json");
    write_json(&review_path, &review)?;
    for rule in &review.rules {
        say(out, format!("  {}: {}", rule.id, rule.summary))?;
    }
    for d in &review.diagnostics {
        say(out, format!("  {:?}: {}", d.severity, d.message))?;
    }
    if review.is_blocked() {
        say(out, format!("The type-check found {} blocking problem(s), so this mapping cannot be approved. Fix the proposal and run the guide again.", review.blocking_diagnostics))?;
        return Err(Error::Invalid {
            message: format!(
                "review found {} blocking type-check diagnostic(s); nothing was approved",
                review.blocking_diagnostics
            ),
        });
    }
    say(
        out,
        format!(
            "Type-check: ok. The review record was written to {}.",
            review_path.display()
        ),
    )?;

    // 4. Approve.
    step(
        out,
        4,
        "Approve",
        "Approving binds the exact contracts, rules, review record and fixture files by SHA-256. If any of them changes later, the approval becomes stale and verify refuses it. Your name and the time are recorded; they are a local record, not a signature.",
    )?;
    let reviewer = match &options.reviewer {
        Some(name) => name.clone(),
        None => {
            write!(out, "Your name, recorded as the reviewer: ").map_err(io_err)?;
            out.flush().map_err(io_err)?;
            match read_line(input)? {
                Some(name) if !name.is_empty() => name,
                _ => {
                    say(out, "".into())?;
                    return Err(Error::Invalid {
                        message: "a reviewer name is required: type it when asked or pass --reviewer; nothing was approved".into(),
                    });
                }
            }
        }
    };
    let fixture_files = crate::approve::digest_fixture_dir(&options.fixtures)?.len();
    say(
        out,
        format!(
            "Reviewer: {reviewer}. Rules: {}. Fixture files: {fixture_files}.",
            proposal.rules.len()
        ),
    )?;
    let approved = if options.yes {
        say(
            out,
            "Approve this mapping? [y/N] y (given with --yes)".into(),
        )?;
        true
    } else {
        ask_approval(input, out)?
    };
    if !approved {
        say(
            out,
            format!(
                "Not approved. Nothing was approved; the review record stays in {}.",
                dir.display()
            ),
        )?;
        return Err(Error::Unapproved);
    }
    let migration = approve(
        &proposal_path,
        &options.source,
        &options.destination,
        &options.fixtures,
        &review_path,
        &reviewer,
        options.note.as_deref(),
        options.approved_at.as_deref(),
    )?;
    let migration_path = dir.join("migration.json");
    write_json(&migration_path, &migration)?;
    say(
        out,
        format!(
            "Approved by {} at {}. Migration written to {}.",
            migration.approval.reviewer,
            migration.approval.approved_at,
            migration_path.display()
        ),
    )?;

    // 5. Verify.
    step(
        out,
        5,
        "Verify against the examples",
        "Each catalogued example is checked against the old contract, the approved rules are applied, and the result is checked against the new contract. An example that is meant to be refused passes when it is refused for the expected reason.",
    )?;
    let verify = verify_paths_report(
        &migration_path,
        &proposal_path,
        &options.source,
        &options.destination,
        &options.fixtures,
    )?;
    let verify_path = dir.join("verify.json");
    write_json(&verify_path, &verify)?;
    let passed = verify.outcomes.iter().filter(|o| o.passed).count();
    for o in &verify.outcomes {
        let mark = if o.passed { "ok  " } else { "FAIL" };
        let why = o
            .message
            .as_deref()
            .map(|m| format!(" - {m}"))
            .unwrap_or_default();
        say(
            out,
            format!(
                "  {mark} {}: expected {}, got {}{why}",
                o.id, o.expect, o.actual
            ),
        )?;
    }
    say(
        out,
        format!(
            "{passed} of {} example(s) passed. Verify result written to {}.",
            verify.outcomes.len(),
            verify_path.display()
        ),
    )?;

    // 6. Report.
    step(
        out,
        6,
        "Write the report",
        "An offline HTML dashboard, the same data as JSON, and a Markdown summary. They open without a network connection.",
    )?;
    let report = build_report(
        &options.source,
        &options.destination,
        Some(&proposal_path),
        Some(&migration_path),
        Some(&options.fixtures),
        Some(&verify),
    )?;
    let (html, json, md) = (
        dir.join("report.html"),
        dir.join("report.json"),
        dir.join("report.md"),
    );
    write_report_pair(&report, &json, &html)?;
    write_markdown(&md, &report)?;
    let stable = report.verify.as_ref().is_some_and(|v| v.stable);
    // Two lines, each through the escaping printer (which shows a line break
    // inside one line as `\n`).
    say(out, format!("Report: {}", html.display()))?;
    say(
        out,
        format!("Verdict: {}.", if stable { "stable" } else { "not stable" }),
    )?;
    match first_failure(&verify) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(text: &str) -> (bool, String) {
        let mut input = std::io::Cursor::new(text.as_bytes().to_vec());
        let mut out = Vec::new();
        let ok = ask_approval(&mut input, &mut out).unwrap();
        (ok, String::from_utf8(out).unwrap())
    }

    #[test]
    fn only_an_explicit_yes_approves() {
        for yes in ["y\n", "Y\n", "yes\n", " YES \r\n"] {
            let (ok, prompt) = answer(yes);
            assert!(ok, "{yes:?}");
            assert!(prompt.starts_with("Approve this mapping? [y/N] "));
        }
        for no in ["\n", "n\n", "no\n", "yy\n", "sure\n", ""] {
            assert!(!answer(no).0, "{no:?} must not approve");
        }
    }
}
