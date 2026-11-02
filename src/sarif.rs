//! SARIF 2.1.0 output for a verify report. Only failing fixtures become
//! results. Each result names a real rule (one per outcome class), points at
//! the fixture file relative to the fixtures directory, and carries the
//! located field position when verify could find it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::error::Result;
use crate::verify::{FixtureOutcome, VerifyReport};

pub const TOOL_NAME: &str = "contract-flightplan";
pub const RULE_PREFIX: &str = "cfp.fixture";

/// Stable rule id for a fixture whose actual outcome differs from the
/// catalogued expectation, keyed by the actual outcome class.
pub fn rule_id(outcome: &FixtureOutcome) -> String {
    format!("{RULE_PREFIX}.{}", outcome.actual)
}

fn rule_description(actual: &str) -> String {
    match actual {
        "success" => "The migration succeeded on a fixture the catalog expected to fail.".into(),
        "collision" => "A rule wrote into a field that already exists.".into(),
        "missing" => "A field the rule needs is absent from the fixture.".into(),
        "explicit_null" => "A field is explicitly null and the rule does not replace null.".into(),
        "unknown_enum" => "A value is outside the mapped enum and the policy is fail.".into(),
        "malformed" => "A value has the wrong type, format, or shape for the rule.".into(),
        "schema_validation" => {
            "The fixture or the transformed output violates the contract schema.".into()
        }
        "unsupported" => "The fixture touches a construct outside the supported subset.".into(),
        "bound" => "A resource bound (size, depth, count) was exceeded.".into(),
        other => format!("Fixture outcome {other} differed from the catalogued expectation."),
    }
}

/// The SARIF `uri` of a fixture: its catalog path, which must stay inside the
/// fixtures folder (a URL, an absolute or drive path or a `..` part is refused),
/// with every byte outside RFC 3986's unreserved set percent-encoded.
fn fixture_uri(path: &str) -> Result<String> {
    let key = crate::approve::fixture_key(path)?;
    let encode = |segment: &str| {
        segment
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    Ok(key.split('/').map(encode).collect::<Vec<_>>().join("/"))
}

pub fn from_verify(report: &VerifyReport) -> Result<Value> {
    let mut results = Vec::new();
    let mut rules: BTreeMap<String, Value> = BTreeMap::new();
    for outcome in &report.outcomes {
        if outcome.passed {
            continue;
        }
        let id = rule_id(outcome);
        rules.entry(id.clone()).or_insert_with(|| {
            json!({
                "id": id,
                "name": format!("FixtureOutcome{}", pascal(&outcome.actual)),
                "shortDescription": {"text": format!("fixture outcome {}", outcome.actual)},
                "fullDescription": {"text": rule_description(&outcome.actual)},
                "defaultConfiguration": {"level": "error"}
            })
        });
        let mut physical = json!({
            "artifactLocation": {
                "uri": fixture_uri(&outcome.path)?,
                "uriBaseId": "FIXTURES"
            }
        });
        if let Some(loc) = &outcome.location {
            physical["region"] = json!({
                "startLine": loc.line,
                "startColumn": loc.column
            });
        }
        let mut result = json!({
            "ruleId": id,
            "level": "error",
            "message": {
                "text": format!(
                    "fixture {} expected {} but got {}{}",
                    outcome.id,
                    outcome.expect,
                    outcome.actual,
                    outcome
                        .message
                        .as_deref()
                        .map(|m| format!(": {m}"))
                        .unwrap_or_default()
                )
            },
            "locations": [{"physicalLocation": physical}],
            "properties": {
                "fixtureId": outcome.id,
                "expected": outcome.expect,
                "actual": outcome.actual,
                "migrationId": report.migration_id
            }
        });
        if let Some(field) = &outcome.field {
            result["properties"]["field"] = json!(field);
        }
        results.push(result);
    }
    Ok(json!({
        "version": "2.1.0",
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "runs": [{
            "tool": {
                "driver": {
                    "name": TOOL_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules.into_values().collect::<Vec<_>>()
                }
            },
            "originalUriBaseIds": {
                "FIXTURES": {"description": {"text": "The --fixtures directory passed to cfp verify."}}
            },
            "invocations": [{
                "executionSuccessful": report.stable,
                "properties": {"migrationId": report.migration_id, "stable": report.stable}
            }],
            "results": results
        }]
    }))
}

fn pascal(text: &str) -> String {
    text.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approve::{Approval, BoundDigests};
    use crate::verify::TextLocation;

    fn report(outcomes: Vec<FixtureOutcome>) -> VerifyReport {
        let stable = outcomes.iter().all(|o| o.passed);
        VerifyReport {
            schema_version: "1".into(),
            kind: "verify".into(),
            migration_id: "m".into(),
            approval: Approval {
                reviewer: "r".into(),
                approved_at: "2026-09-22T00:00:00Z".into(),
                note: None,
                bound_digests: BoundDigests {
                    proposal: "sha256:p".into(),
                    rules: "sha256:r".into(),
                    source_contract: "sha256:s".into(),
                    destination_contract: "sha256:d".into(),
                    fixtures: BTreeMap::new(),
                    review_record: "sha256:v".into(),
                },
                identity_boundary: "local".into(),
            },
            outcomes,
            stable,
            verified_at: None,
        }
    }

    fn outcome(id: &str, expect: &str, actual: &str, passed: bool) -> FixtureOutcome {
        FixtureOutcome {
            id: id.into(),
            path: format!("{id}.json"),
            expect: expect.into(),
            actual: actual.into(),
            passed,
            before: json!({}),
            after: None,
            lossiness: vec![],
            message: Some("boom".into()),
            field: Some("id".into()),
            location: Some(TextLocation { line: 3, column: 5 }),
        }
    }

    #[test]
    fn failing_outcomes_become_results_with_real_rules_and_regions() {
        let sarif = from_verify(&report(vec![
            outcome("ok", "success", "success", true),
            outcome("bad", "success", "collision", false),
        ]))
        .unwrap();
        let run = &sarif["runs"][0];
        assert_eq!(run["results"].as_array().unwrap().len(), 1);
        assert_eq!(run["results"][0]["ruleId"], "cfp.fixture.collision");
        assert_eq!(
            run["results"][0]["locations"][0]["physicalLocation"]["region"]["startLine"],
            3
        );
        assert_eq!(
            run["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "bad.json"
        );
        assert_eq!(
            run["tool"]["driver"]["rules"][0]["id"],
            "cfp.fixture.collision"
        );
        assert_eq!(run["invocations"][0]["executionSuccessful"], false);
    }

    #[test]
    fn sarif_uri_is_a_percent_encoded_relative_fixture_path() {
        let mut odd = outcome("odd", "success", "collision", false);
        odd.path = "sub dir/a#1?.json".into();
        let sarif = from_verify(&report(vec![odd])).unwrap();
        assert_eq!(
            sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]
                ["uri"],
            "sub%20dir/a%231%3F.json"
        );
        // The drive path is built at run time so the source holds no local path.
        let drive = format!("{}:/x.json", 'C');
        for hostile in ["https://evil.example/x", "../x.json", "/etc/passwd", &drive] {
            let mut bad = outcome("bad", "success", "collision", false);
            bad.path = hostile.to_string();
            assert!(
                from_verify(&report(vec![bad])).is_err(),
                "{hostile} must not become a SARIF uri"
            );
        }
    }

    #[test]
    fn stable_report_has_no_results_and_no_rules() {
        let sarif = from_verify(&report(vec![outcome("ok", "success", "success", true)])).unwrap();
        assert!(sarif["runs"][0]["results"].as_array().unwrap().is_empty());
        assert!(sarif["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
