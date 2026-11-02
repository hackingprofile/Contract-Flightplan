use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Proposal,
    Reviewed,
    Approved,
    Stale,
    Rejected,
    Failed,
    /// Replaced by a newer migration (`cfp supersede`); never executable.
    Superseded,
}

pub fn can_transition(from: Lifecycle, to: Lifecycle) -> bool {
    matches!(
        (from, to),
        (Lifecycle::Proposal, Lifecycle::Reviewed)
            | (Lifecycle::Proposal, Lifecycle::Rejected)
            | (Lifecycle::Reviewed, Lifecycle::Reviewed)
            | (Lifecycle::Reviewed, Lifecycle::Approved)
            | (Lifecycle::Reviewed, Lifecycle::Rejected)
            | (Lifecycle::Approved, Lifecycle::Stale)
            | (Lifecycle::Approved, Lifecycle::Failed)
            | (Lifecycle::Approved, Lifecycle::Reviewed)
            | (Lifecycle::Approved, Lifecycle::Superseded)
    )
}

pub fn transition(from: Lifecycle, to: Lifecycle) -> Result<Lifecycle> {
    if can_transition(from, to) {
        Ok(to)
    } else {
        Err(Error::Invalid {
            message: format!("illegal lifecycle transition {from:?} -> {to:?}"),
        })
    }
}

pub fn must_not_execute(state: Lifecycle) -> Result<()> {
    match state {
        Lifecycle::Approved => Ok(()),
        Lifecycle::Proposal | Lifecycle::Reviewed => Err(Error::Unapproved),
        Lifecycle::Stale => Err(Error::StaleDigest {
            what: "lifecycle".into(),
            expected: "approved".into(),
            actual: "stale".into(),
        }),
        Lifecycle::Rejected | Lifecycle::Failed => Err(Error::Invalid {
            message: format!("{state:?} migrations are not executable"),
        }),
        Lifecycle::Superseded => Err(Error::Invalid {
            message:
                "superseded migrations are not executable; verify the migration that superseded it"
                    .into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_cannot_jump_to_approved() {
        assert!(transition(Lifecycle::Proposal, Lifecycle::Approved).is_err());
    }

    #[test]
    fn rejected_cannot_verify() {
        assert!(must_not_execute(Lifecycle::Rejected).is_err());
        assert!(must_not_execute(Lifecycle::Proposal).is_err());
        assert!(must_not_execute(Lifecycle::Approved).is_ok());
    }
}
