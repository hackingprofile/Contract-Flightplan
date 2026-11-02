use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diff::Compatibility;
use crate::error::{Error, Result};
use crate::proposal::load_json_value;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImportedEvidence {
    pub source: String,
    pub identity: String,
    pub summary: String,
    /// oasdiff severity as given: 3 error, 2 warning, 1 info.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `level` 3 is `breaking`, 1 is `non_breaking`; 2 (a warning) and a
    /// missing or unknown level are `unknown`.
    #[serde(default = "unknown")]
    pub compatibility: Compatibility,
}

fn unknown() -> Compatibility {
    Compatibility::Unknown
}

/// Read oasdiff's `level`: the integers 1-3, or the names info/warning/error.
fn level(change: &Value) -> Option<u8> {
    match change.get("level")? {
        Value::Number(n) => n.as_u64().filter(|l| (1..=3).contains(l)).map(|l| l as u8),
        Value::String(name) => match name.to_ascii_lowercase().as_str() {
            "info" => Some(1),
            "warning" | "warn" => Some(2),
            "error" | "err" => Some(3),
            _ => None,
        },
        _ => None,
    }
}

/// Optional ingestion of an oasdiff-like JSON report. Unknown shapes are rejected,
/// not silently treated as an empty diff.
pub fn ingest_oasdiff(path: &Path) -> Result<Vec<ImportedEvidence>> {
    let value = load_json_value(path)?;
    ingest_oasdiff_value(&value)
}

pub fn ingest_oasdiff_value(value: &Value) -> Result<Vec<ImportedEvidence>> {
    let changes = change_items(value).ok_or_else(|| Error::Parse {
        path: "oasdiff".into(),
        message: "expected a changes array, a changelog array, or an object with a changes array"
            .into(),
    })?;
    let mut out = Vec::new();
    for change in changes {
        let identity = change
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| change.get("operation").and_then(Value::as_str))
            .unwrap_or("unidentified")
            .to_string();
        let summary = change
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| change.get("description").and_then(Value::as_str))
            .or_else(|| change.get("path").and_then(Value::as_str))
            .unwrap_or("(no text)")
            .to_string();
        let level = level(change);
        let text = |key: &str| change.get(key).and_then(Value::as_str).map(str::to_string);
        out.push(ImportedEvidence {
            source: "oasdiff-json".into(),
            identity,
            summary,
            level,
            operation: text("operation"),
            path: text("path"),
            compatibility: match level {
                Some(3) => Compatibility::Breaking,
                Some(1) => Compatibility::NonBreaking,
                _ => Compatibility::Unknown,
            },
        });
    }
    Ok(out)
}

/// Accept the project's `{changes:[...]}` wrapper and the real `oasdiff`
/// changelog JSON, which is a top-level array of change objects.
fn change_items(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Array(items) => Some(items.as_slice()),
        Value::Object(map) => map
            .get("changes")
            .and_then(Value::as_array)
            .map(Vec::as_slice),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_changes_array() {
        let evidence = ingest_oasdiff_value(&json!({
            "changes": [{"id": "api-path-changed", "text": "/v1/users"}]
        }))
        .unwrap();
        assert_eq!(evidence[0].identity, "api-path-changed");
    }

    #[test]
    fn rejects_unknown_shape() {
        assert!(ingest_oasdiff_value(&json!({"breaking": true})).is_err());
    }

    #[test]
    fn accepts_real_oasdiff_changelog_array() {
        let evidence = ingest_oasdiff_value(&json!([
            {
                "id": "request-body-changed",
                "text": "POST /users request body",
                "level": 3,
                "operation": "POST",
                "path": "/users"
            }
        ]))
        .unwrap();
        assert_eq!(evidence[0].identity, "request-body-changed");
        assert_eq!(evidence[0].summary, "POST /users request body");
    }
}
