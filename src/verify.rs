use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approve::{
    check_catalog_paths, digest_fixture_dir, fixture_file, fixture_key, parse_migration_value,
    Migration,
};
use crate::error::{Error, Result};
use crate::mapping::apply_rules;
use crate::parse::{load_openapi, operations, OpenApiDoc};
use crate::proposal::{load_json_value, load_proposal, proposal_digest, rules_digest, Rule};
use crate::schema::validate_instance;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureCatalog {
    pub schema_version: String,
    pub items: Vec<CatalogItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogItem {
    pub id: String,
    pub path: String,
    pub rule_ids: Vec<String>,
    pub expect: String,
    /// What the fixture shows, in one sentence.
    pub description: String,
    /// Version of this fixture's content; bump it when the file changes meaning.
    pub version: String,
    /// The generator seed, when the fixture was drawn by `cfp fixtures --seed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyReport {
    pub schema_version: String,
    pub kind: String,
    pub migration_id: String,
    pub approval: crate::approve::Approval,
    pub outcomes: Vec<FixtureOutcome>,
    pub stable: bool,
    /// When this verify ran (UTC). Absent in results written before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureOutcome {
    pub id: String,
    pub path: String,
    pub expect: String,
    pub actual: String,
    pub passed: bool,
    pub before: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
    /// Lossiness events that actually happened while applying the rules.
    pub lossiness: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The field the failure is about, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Where that field sits in the fixture file, when it could be located.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<TextLocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TextLocation {
    pub line: usize,
    pub column: usize,
}

/// A catalog id names its fixture in reports, SARIF, terminal messages and
/// generated suites, so it is a plain name: no separators, spaces or control
/// characters that could act as a path or a terminal command.
pub fn is_safe_catalog_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !id.bytes().all(|b| b == b'.')
}

pub fn load_catalog(path: &Path) -> Result<FixtureCatalog> {
    parse_catalog(path, &crate::digest::read_bounded_file(path)?)
}

/// A catalog from bytes already read (and digested) by the caller.
pub fn parse_catalog(path: &Path, bytes: &[u8]) -> Result<FixtureCatalog> {
    let text = crate::digest::utf8_text(path, bytes)?;
    let value = crate::strict_json::from_str(&text).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let catalog: FixtureCatalog = serde_json::from_value(value).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let mut ids = std::collections::BTreeSet::new();
    for item in &catalog.items {
        if !is_safe_catalog_id(&item.id) {
            return Err(Error::Parse {
                path: path.display().to_string(),
                message: format!(
                    "catalog item id {:?} must be 1 to 128 characters from A-Z, a-z, 0-9, '.', '_' and '-', and not only dots",
                    item.id
                ),
            });
        }
        if item.description.trim().is_empty() || item.version.trim().is_empty() {
            return Err(Error::Parse {
                path: path.display().to_string(),
                message: format!(
                    "catalog item {:?} needs a non-empty description and version",
                    item.id
                ),
            });
        }
        // Ids name fixtures in reports, SARIF and generated suites: one each.
        if !ids.insert(item.id.as_str()) {
            return Err(Error::Parse {
                path: path.display().to_string(),
                message: format!("catalog item id {:?} is used more than once", item.id),
            });
        }
    }
    Ok(catalog)
}

/// Verify and fail with `UnexpectedOutcome` when any fixture disagrees with
/// its catalogued expectation.
pub fn verify_paths(
    migration_path: &Path,
    proposal_path: &Path,
    source_path: &Path,
    dest_path: &Path,
    fixtures_dir: &Path,
) -> Result<VerifyReport> {
    let report = verify_paths_report(
        migration_path,
        proposal_path,
        source_path,
        dest_path,
        fixtures_dir,
    )?;
    require_stable(report)
}

/// Verify and return the full report even when fixtures fail. `stable` is
/// false in that case; callers decide how to surface it. The CLI writes this
/// report before exiting non-zero so the failure evidence is never lost.
pub fn verify_paths_report(
    migration_path: &Path,
    proposal_path: &Path,
    source_path: &Path,
    dest_path: &Path,
    fixtures_dir: &Path,
) -> Result<VerifyReport> {
    let value = load_json_value(migration_path)?;
    if value.get("kind").and_then(Value::as_str) == Some("proposal") {
        return Err(Error::Unapproved);
    }
    let migration = parse_migration_value(&value)?;
    verify_loaded_report(
        migration_path,
        &value,
        &migration,
        proposal_path,
        source_path,
        dest_path,
        fixtures_dir,
    )
}

/// [`verify_paths_report`] for a migration file already read once: `value` is
/// its JSON and `migration` the parsed form, so a caller that shows the
/// migration verifies exactly the content it shows.
pub fn verify_loaded_report(
    migration_path: &Path,
    value: &Value,
    migration: &Migration,
    proposal_path: &Path,
    source_path: &Path,
    dest_path: &Path,
    fixtures_dir: &Path,
) -> Result<VerifyReport> {
    check_not_superseded(migration_path, value, migration)?;
    verify_migration_report(
        migration,
        proposal_path,
        source_path,
        dest_path,
        fixtures_dir,
    )
}

pub fn verify_migration(
    migration: &Migration,
    proposal_path: &Path,
    source_path: &Path,
    dest_path: &Path,
    fixtures_dir: &Path,
) -> Result<VerifyReport> {
    let report = verify_migration_report(
        migration,
        proposal_path,
        source_path,
        dest_path,
        fixtures_dir,
    )?;
    require_stable(report)
}

/// Verify `migration`, approved from the reviewed proposal at `proposal_path`.
pub fn verify_migration_report(
    migration: &Migration,
    proposal_path: &Path,
    source_path: &Path,
    dest_path: &Path,
    fixtures_dir: &Path,
) -> Result<VerifyReport> {
    if migration.kind != "migration" || migration.approval.reviewer.is_empty() {
        return Err(Error::Unapproved);
    }
    crate::state::must_not_execute(migration.lifecycle)?;
    crate::history::check_executable_history(migration)?;
    check_reviewed_proposal(migration, proposal_path)?;
    let source = load_openapi(source_path)?;
    let dest = load_openapi(dest_path)?;
    check_digests(migration, &source, &dest, fixtures_dir)?;
    let catalog = check_catalog_paths(fixtures_dir, &migration.approval.bound_digests.fixtures)?;
    check_fixture_coverage(&migration.rules, &catalog)?;
    let source_ops = operations(&source)?;
    let dest_ops = operations(&dest)?;
    let mut outcomes = Vec::new();
    for item in &catalog.items {
        outcomes.push(run_catalog_item(
            migration,
            &source,
            &dest,
            &source_ops,
            &dest_ops,
            fixtures_dir,
            item,
        )?);
    }
    // No outcome is no evidence: an empty run is never stable.
    let passed = !outcomes.is_empty() && outcomes.iter().all(|o| o.passed);
    Ok(VerifyReport {
        schema_version: "1".into(),
        kind: "verify".into(),
        migration_id: migration.id.clone(),
        approval: migration.approval.clone(),
        outcomes,
        stable: passed,
        verified_at: Some(crate::digest::now_rfc3339_utc()),
    })
}

/// Most JSON files the superseded check reads in a migration's folder.
const MAX_FOLDER_FILES: usize = 256;

/// `cfp supersede` revokes: a migration is refused while a migration in the
/// same folder names it in `supersedes` (the successor), or an archive copy
/// there (same id, lifecycle `superseded`) carries this history followed by
/// more events. An archive copy edited back to `approved` is the old
/// migration, so it is refused the same way. With both the successor and the
/// archive removed nothing local can tell; keep them beside it.
fn check_not_superseded(migration_path: &Path, value: &Value, migration: &Migration) -> Result<()> {
    let digest = crate::digest::canonical_digest(value)?;
    let history = serde_json::to_value(&migration.history).map_err(|e| Error::Invalid {
        message: e.to_string(),
    })?;
    let folder = match migration_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let this = std::fs::canonicalize(migration_path).ok();
    let entries =
        std::fs::read_dir(folder).map_err(|e| Error::io(folder.display().to_string(), e))?;
    let mut read = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_json = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if !is_json || !path.is_file() || std::fs::canonicalize(&path).ok() == this {
            continue;
        }
        read += 1;
        if read > MAX_FOLDER_FILES {
            return Err(Error::Bound {
                kind: "folder_files".into(),
                location: folder.display().to_string(),
                message: format!(
                    "more than {MAX_FOLDER_FILES} JSON files beside the migration; keep migrations in their own folder"
                ),
            });
        }
        let Ok(other) = load_json_value(&path) else {
            continue;
        };
        if other.get("kind").and_then(Value::as_str) != Some("migration") {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let superseded = |why: String| Error::Invalid {
            message: format!(
                "migration {:?} was superseded: {why}; verify the migration that superseded it",
                migration.id
            ),
        };
        if other.pointer("/supersedes/digest").and_then(Value::as_str) == Some(digest.as_str()) {
            return Err(superseded(format!(
                "{name} (migration {}) supersedes it",
                other.get("id").and_then(Value::as_str).unwrap_or("?")
            )));
        }
        let archived = other.get("id").and_then(Value::as_str) == Some(migration.id.as_str())
            && other.get("lifecycle").and_then(Value::as_str) == Some("superseded");
        if let (true, Some(events), Some(mine)) =
            (archived, other["history"].as_array(), history.as_array())
        {
            if events.len() > mine.len() && events[..mine.len()] == mine[..] {
                return Err(superseded(format!(
                    "the archive copy {name} continues its history with a supersession"
                )));
            }
        }
    }
    Ok(())
}

/// Every rule must be exercised by at least one catalogued fixture, and an
/// empty catalog is refused: a run with no fixture for a rule proves nothing
/// about that rule, so it can never be reported stable.
pub fn check_fixture_coverage(rules: &[Rule], catalog: &FixtureCatalog) -> Result<()> {
    if catalog.items.is_empty() {
        return Err(Error::Invalid {
            message: "the fixture catalog lists no fixtures; every rule needs at least one".into(),
        });
    }
    let uncovered: Vec<&str> = rules
        .iter()
        .filter(|rule| {
            !catalog
                .items
                .iter()
                .any(|item| item.rule_ids.contains(&rule.id))
        })
        .map(|rule| rule.id.as_str())
        .collect();
    if !uncovered.is_empty() {
        return Err(Error::Invalid {
            message: format!(
                "no fixtures in the catalog exercise rule(s) {}; every rule needs at least one",
                uncovered.join(", ")
            ),
        });
    }
    Ok(())
}

/// The migration must be the reviewed proposal it approved: the proposal file's
/// digest is the bound one and its rules are exactly the migration's. A forger
/// who rewrites the rules and recomputes every digest in the migration must
/// then also change the reviewed proposal file, which a code review sees.
pub fn check_reviewed_proposal(migration: &Migration, proposal_path: &Path) -> Result<()> {
    let proposal = load_proposal(proposal_path)?;
    let bound = &migration.approval.bound_digests;
    let reviewed = proposal_digest(&proposal)?;
    if reviewed != bound.proposal {
        return Err(Error::StaleDigest {
            what: "reviewed proposal (it is not the proposal this migration approved)".into(),
            expected: bound.proposal.clone(),
            actual: reviewed,
        });
    }
    let reviewed_rules = rules_digest(&proposal.rules)?;
    let migration_rules = rules_digest(&migration.rules)?;
    if proposal.id != migration.id || reviewed_rules != migration_rules {
        return Err(Error::StaleDigest {
            what: "migration rules (they are not the reviewed proposal's rules)".into(),
            expected: reviewed_rules,
            actual: migration_rules,
        });
    }
    Ok(())
}

/// The error a failing verify report maps to: the first fixture whose actual
/// outcome differs from its catalogued expectation.
pub fn first_failure(report: &VerifyReport) -> Option<Error> {
    report
        .outcomes
        .iter()
        .find(|o| !o.passed)
        .map(|first| Error::UnexpectedOutcome {
            id: first.id.clone(),
            expected: first.expect.clone(),
            actual: first.actual.clone(),
        })
}

fn require_stable(report: VerifyReport) -> Result<VerifyReport> {
    if report.stable {
        return Ok(report);
    }
    Err(first_failure(&report).unwrap_or(Error::Invalid {
        message: "verify report is unstable without a failing outcome".into(),
    }))
}

/// 1-based line and column of the first `"field":` key in a JSON text.
pub fn locate_field(text: &str, field: &str) -> Option<TextLocation> {
    let needle = format!("\"{}\"", field.replace('\\', "\\\\").replace('"', "\\\""));
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(&needle) {
        let start = search_from + rel;
        let after = &text[start + needle.len()..];
        if after.trim_start().starts_with(':') {
            let prefix = &text[..start];
            let line = prefix.matches('\n').count() + 1;
            let column = prefix
                .rsplit('\n')
                .next()
                .map(|l| l.chars().count())
                .unwrap_or(0)
                + 1;
            return Some(TextLocation { line, column });
        }
        search_from = start + needle.len();
    }
    None
}

/// The loaded contracts (digests of the bytes parsed) and the fixtures folder
/// must be what the migration bound.
pub fn check_digests(
    migration: &Migration,
    source: &OpenApiDoc,
    dest: &OpenApiDoc,
    fixtures_dir: &Path,
) -> Result<()> {
    let bound = &migration.approval.bound_digests;
    let (source_hash, dest_hash) = (&source.digest, &dest.digest);
    expect_digest("source_contract", &bound.source_contract, source_hash)?;
    expect_digest(
        "destination_contract",
        &bound.destination_contract,
        dest_hash,
    )?;
    let rules_hash = rules_digest(&migration.rules)?;
    expect_digest("rules", &bound.rules, &rules_hash)?;
    let current_fixtures = digest_fixture_dir(fixtures_dir)?;
    expect_map("fixtures", &bound.fixtures, &current_fixtures)?;
    Ok(())
}

fn expect_digest(what: &str, expected: &str, actual: &str) -> Result<()> {
    if expected != actual {
        return Err(Error::StaleDigest {
            what: what.into(),
            expected: expected.into(),
            actual: actual.into(),
        });
    }
    Ok(())
}

fn expect_map(
    what: &str,
    expected: &BTreeMap<String, String>,
    actual: &BTreeMap<String, String>,
) -> Result<()> {
    if expected != actual {
        return Err(Error::StaleDigest {
            what: what.into(),
            expected: format!("{expected:?}"),
            actual: format!("{actual:?}"),
        });
    }
    Ok(())
}

fn run_catalog_item(
    migration: &Migration,
    source: &OpenApiDoc,
    dest: &OpenApiDoc,
    source_ops: &[crate::parse::OperationView],
    dest_ops: &[crate::parse::OperationView],
    fixtures_dir: &Path,
    item: &CatalogItem,
) -> Result<FixtureOutcome> {
    let path = fixture_file(fixtures_dir, &item.path)?;
    let bytes = crate::digest::read_bounded_file(&path)?;
    // The bytes read must be the bytes approved, not only the folder listing.
    let key = fixture_key(&item.path)?;
    let bound = migration.approval.bound_digests.fixtures.get(&key);
    expect_digest(
        &format!("fixture {key}"),
        bound.map(String::as_str).unwrap_or("<not bound>"),
        &crate::digest::sha256_prefixed(&bytes),
    )?;
    let text = crate::digest::utf8_text(&path, &bytes)?;
    let before: Value = crate::strict_json::from_str(&text).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let rules = select_rules(&migration.rules, &item.rule_ids)?;
    let source_schema = body_schema(source, source_ops, &rules[0], true)?;
    if let Err(err) = validate_instance(&source_schema, &before, &item.id) {
        let actual = err.outcome_name().to_string();
        let passed = actual == item.expect;
        let field = failure_field(&err, &item.id);
        let location = field.as_deref().and_then(|f| locate_field(&text, f));
        return Ok(FixtureOutcome {
            id: item.id.clone(),
            path: item.path.clone(),
            expect: item.expect.clone(),
            actual,
            passed,
            before,
            after: None,
            lossiness: Vec::new(),
            message: Some(err.to_string()),
            field,
            location,
        });
    }
    let applied = apply_rules(&rules, &before);
    let (actual, after, lossiness, message, field) = match applied {
        Ok(result) => {
            let dest_schema = body_schema(dest, dest_ops, &rules[0], false)?;
            match validate_instance(&dest_schema, &result.output, &item.id) {
                Ok(()) => (
                    "success".to_string(),
                    Some(result.output),
                    result.lossiness,
                    None,
                    None,
                ),
                Err(err) => (
                    err.outcome_name().to_string(),
                    Some(result.output),
                    result.lossiness,
                    Some(err.to_string()),
                    failure_field(&err, &item.id),
                ),
            }
        }
        Err(err) => (
            err.outcome_name().to_string(),
            None,
            Vec::new(),
            Some(err.to_string()),
            failure_field(&err, &item.id),
        ),
    };
    let passed = actual == item.expect;
    let location = field.as_deref().and_then(|f| locate_field(&text, f));
    Ok(FixtureOutcome {
        id: item.id.clone(),
        path: item.path.clone(),
        expect: item.expect.clone(),
        actual,
        passed,
        before,
        after,
        lossiness,
        message,
        field,
        location,
    })
}

/// Schema validation reports the field as `fixture-id/field`; mapping errors
/// report the bare field. Normalise both to the bare field name.
fn failure_field(err: &Error, item_id: &str) -> Option<String> {
    let raw = err.field_name()?;
    let stripped = raw
        .strip_prefix(item_id)
        .map(|rest| rest.trim_start_matches('/'))
        .unwrap_or(raw);
    if stripped.is_empty() {
        return None;
    }
    Some(stripped.split('/').next().unwrap_or(stripped).to_string())
}

fn select_rules(rules: &[Rule], ids: &[String]) -> Result<Vec<Rule>> {
    let mut out = Vec::new();
    for id in ids {
        match rules.iter().find(|r| &r.id == id) {
            Some(rule) => out.push(rule.clone()),
            None => {
                return Err(Error::Unknown {
                    what: "rule".into(),
                    location: id.clone(),
                    message: "catalog references a rule that is not in the migration".into(),
                });
            }
        }
    }
    if out.is_empty() {
        return Err(Error::Invalid {
            message: "catalog item must name at least one rule".into(),
        });
    }
    Ok(out)
}

pub(crate) fn body_schema(
    doc: &OpenApiDoc,
    ops: &[crate::parse::OperationView],
    rule: &Rule,
    source_side: bool,
) -> Result<crate::schema::ResolvedSchema> {
    let target = if source_side {
        &rule.source
    } else {
        &rule.destination
    };
    let op = ops
        .iter()
        .find(|candidate| {
            candidate.identity.method == target.operation.method
                && candidate.identity.path == target.operation.path
        })
        .ok_or_else(|| Error::Unknown {
            what: "operation".into(),
            location: format!("{} {}", target.operation.method, target.operation.path),
            message: if source_side {
                "source operation not found".into()
            } else {
                "destination operation not found".into()
            },
        })?;
    let body = match target.location.as_str() {
        "request" => op.request.as_ref().ok_or_else(|| Error::Invalid {
            message: format!(
                "rule {} location request has no JSON body",
                if source_side { "source" } else { "destination" }
            ),
        })?,
        "response" => {
            let status = target.response_status.as_deref().unwrap_or("200");
            op.responses.get(status).ok_or_else(|| Error::Invalid {
                message: format!("rule response {status} not found"),
            })?
        }
        other => {
            return Err(Error::Unsupported {
                construct: "location".into(),
                location: other.into(),
                message: "location must be request or response".into(),
            });
        }
    };
    let _ = doc;
    Ok(body.resolved.clone())
}

pub fn review_proposal(
    proposal_path: &Path,
    source_path: &Path,
    dest_path: &Path,
) -> Result<ReviewDocument> {
    let proposal = load_proposal(proposal_path)?;
    let source = load_openapi(source_path)?;
    let dest = load_openapi(dest_path)?;
    let diagnostics = crate::typecheck::check(&proposal, &source, &dest)?;
    let blocking = diagnostics.iter().filter(|d| d.is_error()).count();
    let diff = crate::diff::compare_docs(&source, &dest)?;
    let mut ambiguity = Vec::new();
    for note in &diff.unknown {
        ambiguity.push(format!("{}: {}", note.location, note.message));
    }
    for op in &diff.operations {
        for note in &op.notes {
            if note.contains("ambiguous") || note.contains("assumption") {
                ambiguity.push(format!(
                    "{} {}: {note}",
                    op.identity.method, op.identity.path
                ));
            }
        }
    }
    // A rule with other candidates is ambiguous by construction; so is a rule
    // whose body carries a construct the tool cannot read.
    for rule in &proposal.rules {
        let others: Vec<String> = proposal
            .alternatives
            .iter()
            .filter(|a| a.rule_id == rule.id)
            .map(|a| a.parameters.to_string())
            .collect();
        if !others.is_empty() {
            ambiguity.push(format!(
                "rule {} ({} {}): {} other candidate(s): {}",
                rule.id,
                rule.source.field,
                rule.operation,
                others.len(),
                others.join(", ")
            ));
        }
        let place = match &rule.source.response_status {
            Some(status) => format!("responses/{}", crate::parse::pointer_escape(status)),
            None => "requestBody".into(),
        };
        let prefix = format!(
            "/paths/{}/{}/{place}",
            crate::parse::pointer_escape(&rule.source.operation.path),
            rule.source.operation.method.to_ascii_lowercase()
        );
        for note in diff.unsupported.iter().chain(diff.unknown.iter()) {
            if note.location.starts_with(&prefix) {
                ambiguity.push(format!(
                    "rule {} reads a body with {} at {}",
                    rule.id, note.construct, note.location
                ));
            }
        }
    }
    let mut explanations = Vec::new();
    for rule in &proposal.rules {
        let mapping = crate::proposal::decode_mapping(&rule.operation, &rule.parameters)?;
        explanations.push(RuleExplanation {
            id: rule.id.clone(),
            operation: rule.operation.clone(),
            summary: explain(&mapping, rule),
            lossiness: rule.lossiness.clone(),
            reversibility: rule.reversibility.clone(),
            evidence: rule.evidence.clone(),
            preconditions: rule.preconditions.clone(),
            postconditions: rule.postconditions.clone(),
        });
    }
    crate::state::transition(
        crate::state::Lifecycle::Proposal,
        crate::state::Lifecycle::Reviewed,
    )?;
    Ok(ReviewDocument {
        schema_version: "1".into(),
        kind: "review".into(),
        proposal_digest: proposal_digest(&proposal)?,
        proposal_id: proposal.id,
        approval_state: "not_approved".into(),
        lifecycle: crate::state::Lifecycle::Reviewed,
        typecheck: if blocking == 0 {
            "ok".into()
        } else {
            "failed".into()
        },
        blocking_diagnostics: blocking,
        diff,
        rules: explanations,
        diagnostics,
        ambiguity,
        alternatives: proposal.alternatives.clone(),
        assumptions: proposal.assumptions.clone(),
        provider_boundary: "Providers remain disabled. Review does not call a model.".into(),
    })
}

impl ReviewDocument {
    /// True when at least one error-severity diagnostic blocks approval.
    pub fn is_blocked(&self) -> bool {
        self.blocking_diagnostics > 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDocument {
    pub schema_version: String,
    pub kind: String,
    pub proposal_id: String,
    pub proposal_digest: String,
    pub approval_state: String,
    pub lifecycle: crate::state::Lifecycle,
    /// `ok` or `failed`. `failed` means approve will refuse this proposal.
    #[serde(default)]
    pub typecheck: String,
    #[serde(default)]
    pub blocking_diagnostics: usize,
    pub diff: crate::diff::DiffDocument,
    pub rules: Vec<RuleExplanation>,
    #[serde(default)]
    pub diagnostics: Vec<crate::typecheck::Diagnostic>,
    #[serde(default)]
    pub ambiguity: Vec<String>,
    #[serde(default)]
    pub alternatives: Vec<crate::proposal::Alternative>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub provider_boundary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleExplanation {
    pub id: String,
    pub operation: String,
    pub summary: String,
    pub lossiness: String,
    pub reversibility: String,
    pub evidence: String,
    pub preconditions: Vec<String>,
    pub postconditions: Vec<String>,
}

fn explain(mapping: &crate::proposal::MappingOp, rule: &Rule) -> String {
    match mapping {
        crate::proposal::MappingOp::Rename { from, to } => format!(
            "Rename {from} to {to} on {} {} {}. Collision fails if {to} already exists. Missing {from} fails.",
            rule.source.operation.method, rule.source.operation.path, rule.source.location
        ),
        crate::proposal::MappingOp::Copy { from, to } => format!(
            "Copy {from} to {to} on {} {} {}. Source field remains. Collision fails if {to} already exists.",
            rule.source.operation.method, rule.source.operation.path, rule.source.location
        ),
        crate::proposal::MappingOp::Default { field, value } => format!(
            "Insert default {value} for missing {field} on {} {}. Explicit null is preserved and fails this rule; it is not replaced.",
            rule.source.operation.method, rule.source.operation.path
        ),
        crate::proposal::MappingOp::EnumMap { field, map, unmapped } => {
            let policy = match unmapped {
                crate::proposal::UnmappedPolicy::Fail => "unmapped values fail".to_string(),
                crate::proposal::UnmappedPolicy::Const(v) => {
                    format!("unmapped values use explicit approved constant {v}")
                }
            };
            format!(
                "Map enum {field} with {} entries ({policy}).",
                map.len()
            )
        }
        crate::proposal::MappingOp::Drop { field } => format!(
            "Drop {field} on {} {}. Missing {field} fails. Destination must not require the field.",
            rule.source.operation.method, rule.source.operation.path
        ),
        crate::proposal::MappingOp::RequiredField { field } => format!(
            "Require {field} on {} {}. Missing fails; explicit null fails.",
            rule.source.operation.method, rule.source.operation.path
        ),
        crate::proposal::MappingOp::SafeCast { field, from, to, format } => format!(
            "Safe-cast {field} from {from} to {to}{} on {} {}. Overflow and malformed values fail.",
            format.as_deref().map(|f| format!(" format={f}")).unwrap_or_default(),
            rule.source.operation.method, rule.source.operation.path
        ),
        crate::proposal::MappingOp::UnitConvert { field, from_unit, to_unit, factor_num, factor_den, rounding } => format!(
            "Convert {field} from {from_unit} to {to_unit} using {factor_num}/{factor_den} rounding {rounding:?}."
        ),
        crate::proposal::MappingOp::Split { field, into, separator } => format!(
            "Split {field} on {separator:?} into {}.",
            into.join(", ")
        ),
        crate::proposal::MappingOp::Merge { fields, into, separator } => format!(
            "Merge {} into {into} with {separator:?}.",
            fields.join(", ")
        ),
        crate::proposal::MappingOp::ArrayMap { field, map, unmapped } => {
            let policy = match unmapped {
                crate::proposal::UnmappedPolicy::Fail => "unmapped values fail",
                crate::proposal::UnmappedPolicy::Const(_) => "unmapped values use approved const",
            };
            format!("Map array {field} with {} entries ({policy}).", map.len())
        }
        crate::proposal::MappingOp::ArrayFilter { field, drop_null } => format!(
            "Filter array {field} drop_null={drop_null}."
        ),
        crate::proposal::MappingOp::Pagination { from_items, from_token, to_data, to_cursor } => format!(
            "Reshape page envelope {from_items}/{from_token} to {to_data}/{to_cursor}."
        ),
    }
}
