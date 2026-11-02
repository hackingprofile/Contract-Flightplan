use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::proposal::{load_proposal, Proposal};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: String,
    pub enabled: bool,
    pub outbound: bool,
    pub consent_required: bool,
    pub status: String,
    pub notes: String,
}

pub fn catalog() -> Vec<ProviderDescriptor> {
    vec![
        ProviderDescriptor {
            id: "none".into(),
            enabled: true,
            outbound: false,
            consent_required: false,
            status: "available".into(),
            notes: "Does not generate proposals. Deterministic core stays provider-disabled."
                .into(),
        },
        ProviderDescriptor {
            id: "file".into(),
            enabled: true,
            outbound: false,
            consent_required: false,
            status: "available".into(),
            notes: "Validates a local proposal file. No network.".into(),
        },
        ProviderDescriptor {
            id: "example".into(),
            enabled: true,
            outbound: false,
            consent_required: false,
            status: "available".into(),
            notes: "Deterministic local heuristic: same-schema removed/added properties become rename candidates, with alternatives and assumptions. No model, no network.".into(),
        },
        ProviderDescriptor {
            id: "openai".into(),
            enabled: false,
            outbound: true,
            consent_required: true,
            status: "not_enabled".into(),
            notes:
                "Preview only. Outbound call cannot occur before explicit consent and redaction."
                    .into(),
        },
        ProviderDescriptor {
            id: "anthropic".into(),
            enabled: false,
            outbound: true,
            consent_required: true,
            status: "not_enabled".into(),
            notes:
                "Preview only. Outbound call cannot occur before explicit consent and redaction."
                    .into(),
        },
    ]
}

/// One selected local file as it would appear in an outbound request:
/// identity (name, size, digest) plus the redacted shape of its content.
/// Values are replaced with type markers; keys and structure are kept so the
/// reviewer can see exactly which fields would leave the machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedFile {
    pub file: String,
    pub bytes: usize,
    pub sha256: String,
    pub shape: serde_json::Value,
}

pub fn consent_preview(provider_id: &str, selection: &[PathBuf]) -> Result<serde_json::Value> {
    let provider = catalog()
        .into_iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| Error::Unknown {
            what: "provider".into(),
            location: provider_id.into(),
            message: "unknown provider id".into(),
        })?;
    let mut selected = Vec::new();
    for path in selection {
        selected.push(describe_selection(path)?);
    }
    if provider.outbound && !provider.enabled {
        return Ok(serde_json::json!({
            "provider": provider,
            "selected_files": selected.len(),
            "would_send": selected,
            "redaction": "scalar values replaced with type markers; object keys and array lengths kept",
            "blocked": true,
            "outbound_request_issued": false,
            "message": if selection.is_empty() {
                "provider is not enabled; pass --select <file> to preview what a request would contain. No outbound request is issued."
            } else {
                "provider is not enabled; this is the redacted content a request would contain. No outbound request is issued."
            }
        }));
    }
    Ok(serde_json::json!({
        "provider": provider,
        "selected_files": selected.len(),
        "would_send": [],
        "blocked": false,
        "outbound_request_issued": false,
        "message": "local provider; selected files stay on this machine"
    }))
}

fn describe_selection(path: &Path) -> Result<SelectedFile> {
    let bytes = crate::digest::read_bounded_file(path)?;
    let text = crate::digest::utf8_text(path, &bytes)?;
    let shape = match crate::parse::parse_document(path, &text) {
        Ok(value) => redact(&value),
        Err(_) => serde_json::json!("<opaque text>"),
    };
    Ok(SelectedFile {
        file: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        bytes: bytes.len(),
        sha256: crate::digest::sha256_prefixed(&bytes),
        shape,
    })
}

/// Replace every scalar with a type marker. Keys, nesting and array lengths
/// survive so the shape is reviewable without exposing values.
pub fn redact(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Null => Value::String("<null>".into()),
        Value::Bool(_) => Value::String("<boolean>".into()),
        Value::Number(_) => Value::String("<number>".into()),
        Value::String(_) => Value::String("<string>".into()),
        Value::Array(items) => {
            let mut out = Vec::new();
            if let Some(first) = items.first() {
                out.push(redact(first));
            }
            if items.len() > 1 {
                out.push(Value::String(format!("<{} more items>", items.len() - 1)));
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(k.clone(), redact(v));
            }
            Value::Object(out)
        }
    }
}

/// Validate a local proposal file. When it is refused and `quarantine` is
/// given, the file is copied there as `<sha256>.json` beside
/// `<sha256>.diagnostics.json`, and the refusal becomes `quarantined`
/// (exit 1). Nothing else is written.
pub fn propose_file(path: &Path, quarantine: Option<&Path>) -> Result<Proposal> {
    match load_proposal(path) {
        Ok(proposal) => Ok(proposal),
        Err(err) => {
            let Some(dir) = quarantine else {
                return Err(err);
            };
            // Only what can be read within the input bounds is copied; a file
            // that cannot (too large, not a regular file) is refused as it is.
            let bytes = crate::digest::read_bounded_file(path)?;
            let digest = crate::digest::sha256_prefixed(&bytes);
            let hex = digest.trim_start_matches("sha256:").to_string();
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.display().to_string(), e))?;
            let copy = dir.join(format!("{hex}.json"));
            crate::approve::write_output(&copy, &bytes)?;
            let report = serde_json::json!({
                "file": path.file_name().map(|n| n.to_string_lossy().into_owned()),
                "sha256": digest,
                "outcome": err.outcome_name(),
                "diagnostics": [err.to_string()],
            });
            crate::approve::write_json(&dir.join(format!("{hex}.diagnostics.json")), &report)?;
            Err(Error::Quarantined {
                path: crate::parse::portable_path(&copy),
                message: err.to_string(),
            })
        }
    }
}

/// What a provider is given: the contracts and the selected inputs by
/// digest and redacted shape, and the digest of their diff. Never raw values.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProposalRequest {
    pub source_contract: crate::approve::DigestedFile,
    pub destination_contract: crate::approve::DigestedFile,
    pub selected: Vec<SelectedFile>,
    pub diff_digest: String,
}

impl ProposalRequest {
    pub fn new(source: &Path, destination: &Path, selection: &[PathBuf]) -> Result<Self> {
        let diff = crate::diff::compare_paths(source, destination)?;
        let diff_value = serde_json::to_value(&diff).map_err(|e| Error::Invalid {
            message: e.to_string(),
        })?;
        let contract = |path: &Path| -> Result<crate::approve::DigestedFile> {
            Ok(crate::approve::DigestedFile {
                path: crate::parse::portable_path(path),
                digest: crate::digest::file_digest(path)?,
            })
        };
        Ok(Self {
            source_contract: contract(source)?,
            destination_contract: contract(destination)?,
            selected: selection
                .iter()
                .map(|p| describe_selection(p))
                .collect::<Result<_>>()?,
            diff_digest: crate::digest::canonical_digest(&diff_value)?,
        })
    }
}

/// What a provider returns: a candidate proposal (unchecked JSON) and its
/// own diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProposalResponse {
    pub proposal: serde_json::Value,
    pub diagnostics: Vec<String>,
}

/// A response that passed every proposal check.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    pub proposal: Proposal,
    pub diagnostics: Vec<String>,
}

pub trait ProposalProvider {
    fn id(&self) -> &'static str;
    fn enabled(&self) -> bool;
    fn propose(&self, request: &ProposalRequest) -> Result<ProposalResponse>;
}

/// Run a provider and accept its response only through the same checks a
/// file proposal gets: no approval fields, known operations, well-formed
/// parameters, no unknown fields. A disabled provider is never called.
pub fn run_provider(
    provider: &dyn ProposalProvider,
    request: &ProposalRequest,
) -> Result<Accepted> {
    if !provider.enabled() {
        return Err(Error::Unsupported {
            construct: "provider".into(),
            location: provider.id().into(),
            message: "provider is not enabled".into(),
        });
    }
    let response = provider.propose(request)?;
    Ok(Accepted {
        proposal: crate::proposal::parse_proposal_value(&response.proposal)?,
        diagnostics: response.diagnostics,
    })
}

pub struct NoneProvider;
impl ProposalProvider for NoneProvider {
    fn id(&self) -> &'static str {
        "none"
    }
    fn enabled(&self) -> bool {
        true
    }
    fn propose(&self, _request: &ProposalRequest) -> Result<ProposalResponse> {
        Err(Error::Invalid {
            message: "provider mode none does not generate proposals; use --mode file --proposal or --mode example".into(),
        })
    }
}

/// Reads a local proposal file (strict JSON).
pub struct FileProvider {
    pub path: PathBuf,
}
impl ProposalProvider for FileProvider {
    fn id(&self) -> &'static str {
        "file"
    }
    fn enabled(&self) -> bool {
        true
    }
    fn propose(&self, _request: &ProposalRequest) -> Result<ProposalResponse> {
        Ok(ProposalResponse {
            proposal: crate::proposal::load_json_value(&self.path)?,
            diagnostics: Vec::new(),
        })
    }
}

/// A deterministic local proposer with no model. In each body it pairs a
/// removed property with an added property of the same schema as a rename,
/// lists every other same-schema pairing as an alternative, and states the
/// heuristic as assumptions. Its output is a proposal like any other: it is
/// reviewed, type-checked and approved by a person before anything runs.
pub struct ExampleLocalProvider {
    pub source: PathBuf,
    pub destination: PathBuf,
}

const EXAMPLE_ASSUMPTIONS: [&str; 3] = [
    "A property removed from a body and a property added to the same body with the same schema (type, format, nullability, enum, items) are one field under a new name.",
    "When several added properties fit, the first by name is proposed and every other one is listed in alternatives; the choice is a guess for the reviewer to confirm.",
    "Only top-level body properties are paired; nested properties and path, query and header parameters are not considered.",
];

impl ProposalProvider for ExampleLocalProvider {
    fn id(&self) -> &'static str {
        "example"
    }
    fn enabled(&self) -> bool {
        true
    }
    fn propose(&self, request: &ProposalRequest) -> Result<ProposalResponse> {
        let fresh = ProposalRequest::new(&self.source, &self.destination, &[])?;
        if fresh.diff_digest != request.diff_digest {
            return Err(Error::StaleDigest {
                what: "provider request diff".into(),
                expected: request.diff_digest.clone(),
                actual: fresh.diff_digest,
            });
        }
        let diff = crate::diff::compare_paths(&self.source, &self.destination)?;
        let mut rules = Vec::new();
        let mut alternatives = Vec::new();
        for op in &diff.operations {
            let mut bodies: Vec<(Option<&String>, &crate::diff::BodyDiff)> = Vec::new();
            if let Some(request_body) = &op.request {
                bodies.push((None, request_body));
            }
            for (status, body) in &op.responses {
                bodies.push((Some(status), body));
            }
            for (status, body) in bodies {
                pair_body(op, status, body, &mut rules, &mut alternatives);
            }
        }
        let proposal = serde_json::json!({
            "schema_version": "1",
            "kind": "proposal",
            "id": "example-local-proposal",
            "source_contract": {"path": request.source_contract.path},
            "destination_contract": {"path": request.destination_contract.path},
            "rules": rules,
            "alternatives": alternatives,
            "assumptions": EXAMPLE_ASSUMPTIONS,
        });
        Ok(ProposalResponse {
            proposal,
            diagnostics: vec![format!(
                "example-local heuristic proposed {} rename(s) with {} alternative(s)",
                rules.len(),
                alternatives.len()
            )],
        })
    }
}

/// The parts of a property schema two candidates must share.
fn schema_key(schema: Option<&serde_json::Value>) -> serde_json::Value {
    let schema = schema.cloned().unwrap_or_default();
    let mut key = serde_json::Map::new();
    for k in ["type", "format", "nullable", "enum", "items"] {
        if let Some(v) = schema.get(k) {
            key.insert(k.into(), v.clone());
        }
    }
    serde_json::Value::Object(key)
}

fn pair_body(
    op: &crate::diff::OperationDiff,
    status: Option<&String>,
    body: &crate::diff::BodyDiff,
    rules: &mut Vec<serde_json::Value>,
    alternatives: &mut Vec<serde_json::Value>,
) {
    use crate::diff::ChangeKind;
    let top = |c: &&crate::diff::Change, kind: ChangeKind| {
        c.kind == kind && !c.field.is_empty() && !c.field.contains(['.', '['])
    };
    let mut removed: Vec<&crate::diff::Change> = body
        .changes
        .iter()
        .filter(|c| top(c, ChangeKind::PropertyRemoved))
        .collect();
    let mut added: Vec<&crate::diff::Change> = body
        .changes
        .iter()
        .filter(|c| top(c, ChangeKind::PropertyAdded))
        .collect();
    removed.sort_by(|a, b| a.field.cmp(&b.field));
    added.sort_by(|a, b| a.field.cmp(&b.field));
    let location = if status.is_some() {
        "response"
    } else {
        "request"
    };
    let tag = status.map_or("request".to_string(), |s| format!("response-{s}"));
    let mut used: Vec<&str> = Vec::new();
    for gone in removed {
        let key = schema_key(gone.source.as_ref());
        let candidates: Vec<&str> = added
            .iter()
            .filter(|c| schema_key(c.destination.as_ref()) == key)
            .map(|c| c.field.as_str())
            .collect();
        let Some(chosen) = candidates.iter().find(|c| !used.contains(c)).copied() else {
            continue;
        };
        used.push(chosen);
        let from = gone.field.as_str();
        let rule_id = format!("example-rename-{tag}-{from}");
        let target = |identity: &crate::parse::OpIdentity, field: &str| {
            let mut t = serde_json::json!({
                "operation": identity,
                "location": location,
                "field": field,
            });
            if let Some(status) = status {
                t["response_status"] = serde_json::json!(status);
            }
            t
        };
        rules.push(serde_json::json!({
            "id": rule_id,
            "operation": "rename",
            "source": target(&op.identity, from),
            "destination": target(&op.destination_identity, chosen),
            "parameters": {"from": from, "to": chosen},
            "preconditions": [format!("{from} is present"), format!("{chosen} is absent")],
            "postconditions": [format!("{chosen} holds the previous {from} value")],
            "evidence": format!(
                "example-local heuristic: {from} was removed and {chosen} added with the same schema in {} {} {tag}",
                op.identity.method, op.identity.path
            ),
            "lossiness": "none",
            "reversibility": "reversible",
            "fixture_refs": [],
        }));
        for other in candidates.iter().filter(|c| **c != chosen) {
            alternatives.push(serde_json::json!({
                "rule_id": rule_id,
                "operation": "rename",
                "parameters": {"from": from, "to": other},
                "why": format!("{other} was also added with the same schema as {from}; the heuristic cannot tell which one continues it"),
            }));
        }
    }
}

pub fn propose(mode: &str, proposal_path: Option<&Path>) -> Result<Proposal> {
    match mode {
        "none" => Err(Error::Invalid {
            message: "provider mode none does not generate proposals; use --mode file --proposal or --mode example"
                .into(),
        }),
        "file" => {
            let path = proposal_path.ok_or_else(|| Error::Invalid {
                message: "file provider requires --proposal".into(),
            })?;
            load_proposal(path)
        }
        "openai" | "anthropic" => Err(Error::Unsupported {
            construct: "provider".into(),
            location: mode.into(),
            message: "hosted providers are not enabled; consent preview cannot send data".into(),
        }),
        other => Err(Error::Unknown {
            what: "provider".into(),
            location: other.into(),
            message: "supported local modes are none, file and example".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_providers_are_preview_only() {
        let openai = catalog().into_iter().find(|p| p.id == "openai").unwrap();
        assert!(!openai.enabled);
        assert!(openai.consent_required);
        let preview = consent_preview("openai", &[]).unwrap();
        assert_eq!(preview["blocked"], true);
        assert_eq!(preview["outbound_request_issued"], false);
        assert!(preview["would_send"].as_array().unwrap().is_empty());
    }

    #[test]
    fn preview_lists_selected_files_with_redacted_shape() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture = root.join("packs/rename-user-id/fixtures/request.positive.json");
        let preview = consent_preview("anthropic", &[fixture]).unwrap();
        assert_eq!(preview["blocked"], true);
        let sent = preview["would_send"].as_array().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["file"], "request.positive.json");
        assert!(sent[0]["sha256"].as_str().unwrap().starts_with("sha256:"));
        let shape = serde_json::to_string(&sent[0]["shape"]).unwrap();
        assert!(shape.contains("<string>") || shape.contains("<number>"));
        let raw = std::fs::read_to_string(
            root.join("packs/rename-user-id/fixtures/request.positive.json"),
        )
        .unwrap();
        let raw_value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for (_, v) in raw_value.as_object().unwrap() {
            if let Some(s) = v.as_str() {
                assert!(
                    !shape.contains(&format!("\"{s}\"")),
                    "value leaked into preview: {s}"
                );
            }
        }
    }

    #[test]
    fn redact_keeps_keys_and_drops_values() {
        let shape = redact(&serde_json::json!({"a": "secret", "b": [1, 2, 3], "c": {"d": true}}));
        assert_eq!(shape["a"], "<string>");
        assert_eq!(shape["b"][0], "<number>");
        assert_eq!(shape["b"][1], "<2 more items>");
        assert_eq!(shape["c"]["d"], "<boolean>");
    }

    #[test]
    fn none_does_not_generate() {
        assert!(propose("none", None).is_err());
        assert!(run_provider(&NoneProvider, &ProposalRequest::default()).is_err());
    }
}
