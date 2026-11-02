use std::io;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("parse error at {path}: {message}")]
    Parse { path: String, message: String },
    #[error("resource bound exceeded ({kind}) at {location}: {message}")]
    Bound {
        kind: String,
        location: String,
        message: String,
    },
    #[error("unsupported {construct} at {location}: {message}")]
    Unsupported {
        construct: String,
        location: String,
        message: String,
    },
    #[error("unknown {what} at {location}: {message}")]
    Unknown {
        what: String,
        location: String,
        message: String,
    },
    #[error("ambiguous operation matching at {location}: {message}")]
    AmbiguousOperation { location: String, message: String },
    #[error("rename collision on field {field} at {location}")]
    Collision { field: String, location: String },
    #[error("missing field {field} at {location}")]
    MissingField { field: String, location: String },
    #[error("explicit null for field {field} at {location} is not replaced by a default")]
    ExplicitNull { field: String, location: String },
    #[error("unknown enum value {value} for field {field} at {location}")]
    UnknownEnum {
        field: String,
        value: String,
        location: String,
    },
    #[error("malformed {kind} for field {field} at {location}: {message}")]
    Malformed {
        kind: String,
        field: String,
        location: String,
        message: String,
    },
    #[error("unknown mapping operation '{op}'")]
    UnknownMappingOperation { op: String },
    #[error("proposal is not executable; approve it first")]
    Unapproved,
    #[error("proposal must not contain approval fields")]
    InjectedApproval,
    #[error("stale digest for {what}: expected {expected}, actual {actual}")]
    StaleDigest {
        what: String,
        expected: String,
        actual: String,
    },
    #[error("schema validation failed at {location}: {message}")]
    SchemaValidation { location: String, message: String },
    #[error("fixture '{id}' expected {expected}, got {actual}")]
    UnexpectedOutcome {
        id: String,
        expected: String,
        actual: String,
    },
    #[error("history check failed: {message}")]
    History { message: String },
    #[error("proposal quarantined at {path}: {message}")]
    Quarantined { path: String, message: String },
    #[error("{message}")]
    Invalid { message: String },
}

impl Error {
    pub fn io(path: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn outcome_name(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "unsupported",
            Self::Unknown { .. } => "unknown",
            Self::AmbiguousOperation { .. } => "ambiguous",
            Self::Collision { .. } => "collision",
            Self::MissingField { .. } => "missing",
            Self::ExplicitNull { .. } => "explicit_null",
            Self::UnknownEnum { .. } => "unknown_enum",
            Self::Malformed { .. } => "malformed",
            Self::UnknownMappingOperation { .. } => "unknown_mapping_operation",
            Self::Unapproved => "unapproved",
            Self::InjectedApproval => "injected_approval",
            Self::StaleDigest { .. } => "stale_digest",
            Self::SchemaValidation { .. } => "schema_validation",
            Self::Bound { .. } => "bound",
            Self::Parse { .. } => "parse",
            Self::Io { .. } => "io",
            Self::UnexpectedOutcome { .. } => "unexpected_outcome",
            Self::History { .. } => "history",
            Self::Quarantined { .. } => "quarantined",
            Self::Invalid { .. } => "invalid",
        }
    }

    /// The JSON field a failure points at, when the failure is about one field.
    pub fn field_name(&self) -> Option<&str> {
        match self {
            Self::Collision { field, .. }
            | Self::MissingField { field, .. }
            | Self::ExplicitNull { field, .. }
            | Self::UnknownEnum { field, .. }
            | Self::Malformed { field, .. } => Some(field.as_str()),
            _ => None,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Unsupported { .. } => 2,
            Self::Io { .. } | Self::Parse { .. } | Self::Bound { .. } | Self::Invalid { .. } => 3,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_uses_exit_two() {
        let err = Error::Unsupported {
            construct: "allOf".into(),
            location: "#/components/schemas/X".into(),
            message: "combinator".into(),
        };
        assert_eq!(err.exit_code(), 2);
        assert_eq!(err.outcome_name(), "unsupported");
    }
}
