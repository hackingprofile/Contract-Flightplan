//! A provider is anything that turns a `ProposalRequest` into a
//! `ProposalResponse`. A test-local mock proves the contract: the request
//! carries only the redacted selection and the diff digest, and every response
//! goes through the same checks as a file proposal before it is a proposal.

use contract_flightplan::provider::{
    run_provider, ProposalProvider, ProposalRequest, ProposalResponse,
};
use contract_flightplan::Result;
use serde_json::{json, Value};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

struct Mock {
    reply: Value,
}

impl ProposalProvider for Mock {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn enabled(&self) -> bool {
        true
    }

    fn propose(&self, request: &ProposalRequest) -> Result<ProposalResponse> {
        // A provider sees the request, never the raw files.
        assert!(request.diff_digest.starts_with("sha256:"));
        Ok(ProposalResponse {
            proposal: self.reply.clone(),
            diagnostics: vec![format!(
                "mock saw {} selected input(s)",
                request.selected.len()
            )],
        })
    }
}

#[test]
fn mock_provider_contract_roundtrip() {
    let pack = root().join("packs/rename-user-id");
    let request = ProposalRequest::new(
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
        &[pack.join("fixtures/request.positive.json")],
    )
    .unwrap();
    // The request is plain data: it round-trips through JSON unchanged, and
    // its selection carries digests and redacted shapes, not values.
    let text = serde_json::to_string(&request).unwrap();
    let back: ProposalRequest = serde_json::from_str(&text).unwrap();
    assert_eq!(back, request);
    assert_eq!(request.selected.len(), 1);
    assert!(request.selected[0].sha256.starts_with("sha256:"));
    assert!(
        !text.contains("\"user-1\""),
        "a selected value leaked: {text}"
    );

    let proposal: Value =
        serde_json::from_str(&std::fs::read_to_string(pack.join("proposal.json")).unwrap())
            .unwrap();
    let accepted = run_provider(
        &Mock {
            reply: proposal.clone(),
        },
        &request,
    )
    .unwrap();
    assert_eq!(accepted.proposal.id, "rename-user-id");
    assert_eq!(
        accepted.diagnostics,
        vec!["mock saw 1 selected input(s)".to_string()]
    );

    // A response is refused exactly like a file proposal would be.
    let mut injected = proposal.clone();
    injected["approval"] = json!({"reviewer": "the-model"});
    assert!(run_provider(&Mock { reply: injected }, &request).is_err());
    let mut unknown = proposal;
    unknown["rules"][0]["operation"] = json!("eval");
    assert!(run_provider(&Mock { reply: unknown }, &request).is_err());
}
