use serde::{Deserialize, Serialize};

use crate::approve::Migration;
use crate::digest::now_rfc3339_utc;
use crate::error::{Error, Result};
use crate::proposal::Proposal;
use crate::state::Lifecycle;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoryEvent {
    pub at: String,
    pub action: String,
    pub actor: String,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Canonical digest of the previous event; `null` on the first. Removing
    /// or editing an event breaks the next event's link.
    #[serde(default)]
    pub prev: Option<String>,
}

pub fn event(
    action: &str,
    actor: &str,
    digest: &str,
    note: Option<&str>,
    at: Option<&str>,
) -> HistoryEvent {
    HistoryEvent {
        at: at.map(str::to_string).unwrap_or_else(now_rfc3339_utc),
        action: action.into(),
        actor: actor.into(),
        digest: digest.into(),
        note: note.map(str::to_string),
        prev: None,
    }
}

/// Canonical digest of one event, the value the next event's `prev` holds.
pub fn event_digest(event: &HistoryEvent) -> Result<String> {
    let value = serde_json::to_value(event).map_err(|e| Error::Invalid {
        message: e.to_string(),
    })?;
    crate::digest::canonical_digest(&value)
}

/// Append `next`, linking it to the current last event.
pub fn append(history: &mut Vec<HistoryEvent>, mut next: HistoryEvent) -> Result<()> {
    next.prev = match history.last() {
        Some(last) => Some(event_digest(last)?),
        None => None,
    };
    history.push(next);
    Ok(())
}

fn history_error(message: String) -> Error {
    Error::History { message }
}

/// Check a migration's history: every link holds, the approval matches the
/// last `approved` event (which commits to every bound digest) and the
/// `reviewed` event right before it, and the migration's rules, fixtures and
/// contracts are the ones bound. These are tamper-evident local records, not
/// signatures: someone who recomputes every digest is caught only by `verify`
/// against the reviewed proposal, which is the documented boundary.
pub fn check_history(migration: &Migration) -> Result<()> {
    let history = &migration.history;
    if history.is_empty() {
        return Err(history_error(
            "the migration's history is empty; an approval must carry its reviewed and approved events".into(),
        ));
    }
    for (i, event) in history.iter().enumerate() {
        let expected = match i {
            0 => None,
            _ => Some(event_digest(&history[i - 1])?),
        };
        if event.prev != expected {
            return Err(history_error(format!(
                "event {} ({}) does not link to the event before it (prev {:?}, expected {:?}); an event was removed, reordered or edited",
                i + 1,
                event.action,
                event.prev,
                expected
            )));
        }
    }
    let approval = &migration.approval;
    let approved = history
        .iter()
        .rposition(|e| e.action == "approved")
        .ok_or_else(|| history_error("no approved event".into()))?;
    let event = &history[approved];
    if event.actor != approval.reviewer || event.at != approval.approved_at {
        return Err(history_error(format!(
            "the approval (reviewer {:?}, at {}) does not match the approved event (actor {:?}, at {})",
            approval.reviewer, approval.approved_at, event.actor, event.at
        )));
    }
    let bound = &approval.bound_digests;
    let committed = crate::approve::approval_digest(bound)?;
    if event.digest != committed {
        return Err(history_error(format!(
            "the approved event commits to {} but the approval's bound digests (proposal, rules, contracts, fixtures, review record) now digest to {committed}; the approved content was edited after approval",
            event.digest
        )));
    }
    // The migration's own copies must be what the approval bound.
    let rules = crate::proposal::rules_digest(&migration.rules)?;
    for (what, expected, actual) in [
        ("rules", &bound.rules, &rules),
        (
            "source_contract",
            &bound.source_contract,
            &migration.source_contract.digest,
        ),
        (
            "destination_contract",
            &bound.destination_contract,
            &migration.destination_contract.digest,
        ),
    ] {
        if expected != actual {
            return Err(Error::StaleDigest {
                what: what.into(),
                expected: expected.clone(),
                actual: actual.clone(),
            });
        }
    }
    if migration.fixtures != bound.fixtures {
        return Err(Error::StaleDigest {
            what: "fixtures (the migration's list differs from the bound one)".into(),
            expected: format!("{:?}", bound.fixtures),
            actual: format!("{:?}", migration.fixtures),
        });
    }
    let reviewed = approved
        .checked_sub(1)
        .map(|i| &history[i])
        .filter(|e| e.action == "reviewed")
        .ok_or_else(|| {
            history_error("the approved event is not preceded by a reviewed event".into())
        })?;
    if reviewed.digest != approval.bound_digests.review_record {
        return Err(history_error(
            "the reviewed event does not carry the bound review record digest".into(),
        ));
    }
    Ok(())
}

/// The check `verify` runs: `check_history`, and the approved event is the
/// last event (nothing was recorded after the approval being executed).
pub fn check_executable_history(migration: &Migration) -> Result<()> {
    check_history(migration)?;
    match migration.history.last() {
        Some(last) if last.action == "approved" => Ok(()),
        Some(last) => Err(history_error(format!(
            "the last event is {:?}, not the approval being executed",
            last.action
        ))),
        None => Err(history_error("the migration's history is empty".into())),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rejection {
    pub schema_version: String,
    pub kind: String,
    pub id: String,
    pub lifecycle: Lifecycle,
    pub reviewer: String,
    pub reason: String,
    pub rejected_at: String,
    pub proposal_digest: String,
    pub history: Vec<HistoryEvent>,
}

pub fn reject_proposal(
    proposal: &Proposal,
    reviewer: &str,
    reason: &str,
    rejected_at: Option<&str>,
) -> Result<Rejection> {
    if reviewer.trim().is_empty() || reason.trim().is_empty() {
        return Err(Error::Invalid {
            message: "reject requires reviewer and reason".into(),
        });
    }
    crate::state::transition(Lifecycle::Proposal, Lifecycle::Rejected)?;
    let digest = crate::proposal::proposal_digest(proposal)?;
    let at = rejected_at
        .map(str::to_string)
        .unwrap_or_else(now_rfc3339_utc);
    Ok(Rejection {
        schema_version: "1".into(),
        kind: "rejection".into(),
        id: proposal.id.clone(),
        lifecycle: Lifecycle::Rejected,
        reviewer: reviewer.trim().to_string(),
        reason: reason.trim().to_string(),
        rejected_at: at.clone(),
        proposal_digest: digest.clone(),
        history: vec![event(
            "rejected",
            reviewer,
            &digest,
            Some(reason),
            Some(&at),
        )],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reject_is_not_executable() {
        let proposal = crate::proposal::parse_proposal_value(&json!({
            "schema_version":"1",
            "kind":"proposal",
            "id":"x",
            "source_contract":{"path":"a.yaml"},
            "destination_contract":{"path":"b.yaml"},
            "rules":[]
        }))
        .unwrap();
        let rejection = reject_proposal(
            &proposal,
            "reviewer",
            "unsafe",
            Some("2026-09-18T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(rejection.lifecycle, Lifecycle::Rejected);
        assert!(crate::state::must_not_execute(rejection.lifecycle).is_err());
    }
}
