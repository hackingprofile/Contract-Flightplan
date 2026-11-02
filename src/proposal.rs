use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::digest::{canonical_digest, read_bounded_file, utf8_text, MAX_RULES};
use crate::error::{Error, Result};
use crate::parse::OpIdentity;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub schema_version: String,
    pub kind: String,
    pub id: String,
    pub source_contract: PathHint,
    pub destination_contract: PathHint,
    pub rules: Vec<Rule>,
    /// Other candidates a proposer considered for a rule; shown at review,
    /// never executed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<Alternative>,
    /// What the proposer assumed; shown at review.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Alternative {
    pub rule_id: String,
    pub operation: String,
    pub parameters: Value,
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PathHint {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub operation: String,
    pub source: Target,
    pub destination: Target,
    pub parameters: Value,
    pub preconditions: Vec<String>,
    pub postconditions: Vec<String>,
    pub evidence: String,
    pub lossiness: String,
    pub reversibility: String,
    pub fixture_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub operation: OpIdentity,
    pub location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_status: Option<String>,
    pub field: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MappingOp {
    Rename {
        from: String,
        to: String,
    },
    Copy {
        from: String,
        to: String,
    },
    Default {
        field: String,
        value: Value,
    },
    EnumMap {
        field: String,
        map: BTreeMap<String, Value>,
        unmapped: UnmappedPolicy,
    },
    Drop {
        field: String,
    },
    RequiredField {
        field: String,
    },
    SafeCast {
        field: String,
        from: String,
        to: String,
        format: Option<String>,
    },
    UnitConvert {
        field: String,
        from_unit: String,
        to_unit: String,
        factor_num: i64,
        factor_den: i64,
        rounding: Rounding,
    },
    Split {
        field: String,
        into: Vec<String>,
        separator: String,
    },
    Merge {
        fields: Vec<String>,
        into: String,
        separator: String,
    },
    ArrayMap {
        field: String,
        map: BTreeMap<String, Value>,
        unmapped: UnmappedPolicy,
    },
    ArrayFilter {
        field: String,
        drop_null: bool,
    },
    Pagination {
        from_items: String,
        from_token: String,
        to_data: String,
        to_cursor: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    Exact,
    Floor,
    Ceil,
    Round,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnmappedPolicy {
    Fail,
    Const(Value),
}

pub fn load_json_value(path: &Path) -> Result<Value> {
    let bytes = read_bounded_file(path)?;
    let text = utf8_text(path, &bytes)?;
    crate::strict_json::from_str(&text).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })
}

pub fn load_proposal(path: &Path) -> Result<Proposal> {
    let value = load_json_value(path)?;
    if value.get("approval").is_some() {
        return Err(Error::InjectedApproval);
    }
    parse_proposal_value(&value)
}

pub fn parse_proposal_value(value: &Value) -> Result<Proposal> {
    if value.get("approval").is_some() {
        return Err(Error::InjectedApproval);
    }
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind != "proposal" {
        return Err(Error::Invalid {
            message: format!("expected kind=proposal, got {kind}"),
        });
    }
    let proposal: Proposal = serde_json::from_value(value.clone()).map_err(|e| Error::Parse {
        path: "proposal".into(),
        message: e.to_string(),
    })?;
    if proposal.schema_version != "1" {
        return Err(Error::Unsupported {
            construct: "schema_version".into(),
            location: "proposal".into(),
            message: format!(
                "unsupported proposal schema_version {}",
                proposal.schema_version
            ),
        });
    }
    if proposal.rules.len() > MAX_RULES {
        return Err(Error::Bound {
            kind: "rules".into(),
            location: "proposal".into(),
            message: format!("proposal has more than {MAX_RULES} rules"),
        });
    }
    for rule in &proposal.rules {
        decode_mapping(&rule.operation, &rule.parameters)?;
    }
    Ok(proposal)
}

pub fn proposal_digest(proposal: &Proposal) -> Result<String> {
    let value = serde_json::to_value(proposal).map_err(|e| Error::Invalid {
        message: e.to_string(),
    })?;
    canonical_digest(&value)
}

pub fn rules_digest(rules: &[Rule]) -> Result<String> {
    let value = serde_json::to_value(rules).map_err(|e| Error::Invalid {
        message: e.to_string(),
    })?;
    canonical_digest(&value)
}

/// The parameter names the published schema allows for each operation.
pub fn allowed_parameters(operation: &str) -> Option<&'static [&'static str]> {
    Some(match operation {
        "rename" | "copy" => &["from", "to"],
        "default" => &["field", "value"],
        "enum_map" | "array_map" => &["field", "map", "unmapped"],
        "drop" | "required_field" => &["field"],
        "safe_cast" => &["field", "from", "to", "format"],
        "unit_conversion" => &[
            "field",
            "from_unit",
            "to_unit",
            "factor_num",
            "factor_den",
            "rounding",
        ],
        "split" => &["field", "into", "separator"],
        "merge" => &["fields", "into", "separator"],
        "array_filter" => &["field", "drop_null"],
        "pagination" => &["from_items", "from_token", "to_data", "to_cursor"],
        _ => return None,
    })
}

pub fn decode_mapping(operation: &str, parameters: &Value) -> Result<MappingOp> {
    if let Some(allowed) = allowed_parameters(operation) {
        let object = parameters.as_object().ok_or_else(|| Error::Invalid {
            message: format!("{operation} rule parameters must be an object"),
        })?;
        if let Some(unknown) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(Error::Invalid {
                message: format!(
                    "{operation} rule has unknown parameter '{unknown}'; allowed: {}",
                    allowed.join(", ")
                ),
            });
        }
    }
    match operation {
        "rename" => {
            let from = required_string(parameters, "from")?;
            let to = required_string(parameters, "to")?;
            Ok(MappingOp::Rename { from, to })
        }
        "copy" => {
            let from = required_string(parameters, "from")?;
            let to = required_string(parameters, "to")?;
            Ok(MappingOp::Copy { from, to })
        }
        "default" => {
            let field = required_string(parameters, "field")?;
            let value = parameters
                .get("value")
                .cloned()
                .ok_or_else(|| Error::Invalid {
                    message: "default rule requires parameters.value".into(),
                })?;
            Ok(MappingOp::Default { field, value })
        }
        "enum_map" => {
            let field = required_string(parameters, "field")?;
            let (map, unmapped) = decode_map_policy(parameters)?;
            Ok(MappingOp::EnumMap {
                field,
                map,
                unmapped,
            })
        }
        "drop" => Ok(MappingOp::Drop {
            field: required_string(parameters, "field")?,
        }),
        "required_field" => Ok(MappingOp::RequiredField {
            field: required_string(parameters, "field")?,
        }),
        "safe_cast" => {
            let format = match parameters.get("format") {
                None => None,
                Some(Value::String(f)) if f == "email" => Some(f.clone()),
                Some(other) => {
                    return Err(Error::Invalid {
                        message: format!(
                            "safe_cast format {other} is not implemented; supported formats: email"
                        ),
                    });
                }
            };
            Ok(MappingOp::SafeCast {
                field: required_string(parameters, "field")?,
                from: required_string(parameters, "from")?,
                to: required_string(parameters, "to")?,
                format,
            })
        }
        "unit_conversion" => Ok(MappingOp::UnitConvert {
            field: required_string(parameters, "field")?,
            from_unit: required_string(parameters, "from_unit")?,
            to_unit: required_string(parameters, "to_unit")?,
            factor_num: required_i64(parameters, "factor_num")?,
            factor_den: required_i64(parameters, "factor_den")?,
            rounding: required_rounding(parameters)?,
        }),
        "split" => Ok(MappingOp::Split {
            field: required_string(parameters, "field")?,
            into: required_string_list(parameters, "into")?,
            separator: required_string(parameters, "separator")?,
        }),
        "merge" => Ok(MappingOp::Merge {
            fields: required_string_list(parameters, "fields")?,
            into: required_string(parameters, "into")?,
            separator: required_string(parameters, "separator")?,
        }),
        "array_map" => {
            let field = required_string(parameters, "field")?;
            let (map, unmapped) = decode_map_policy(parameters)?;
            Ok(MappingOp::ArrayMap {
                field,
                map,
                unmapped,
            })
        }
        "array_filter" => Ok(MappingOp::ArrayFilter {
            field: required_string(parameters, "field")?,
            drop_null: parameters
                .get("drop_null")
                .and_then(Value::as_bool)
                .ok_or_else(|| Error::Invalid {
                    message: "array_filter requires boolean parameters.drop_null".into(),
                })?,
        }),
        "pagination" => Ok(MappingOp::Pagination {
            from_items: required_string(parameters, "from_items")?,
            from_token: required_string(parameters, "from_token")?,
            to_data: required_string(parameters, "to_data")?,
            to_cursor: required_string(parameters, "to_cursor")?,
        }),
        other => Err(Error::UnknownMappingOperation {
            op: other.to_string(),
        }),
    }
}

fn required_string(parameters: &Value, key: &str) -> Result<String> {
    parameters
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Invalid {
            message: format!("mapping parameter '{key}' must be a string"),
        })
}

fn required_i64(parameters: &Value, key: &str) -> Result<i64> {
    parameters
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::Invalid {
            message: format!("mapping parameter '{key}' must be an integer"),
        })
}

fn required_string_list(parameters: &Value, key: &str) -> Result<Vec<String>> {
    let values = parameters
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Invalid {
            message: format!("mapping parameter '{key}' must be an array of strings"),
        })?;
    let mut out: Vec<String> = Vec::new();
    for value in values {
        let text = value.as_str().ok_or_else(|| Error::Invalid {
            message: format!("mapping parameter '{key}' must contain only strings"),
        })?;
        if text.is_empty() {
            return Err(Error::Invalid {
                message: format!("mapping parameter '{key}' must not contain empty names"),
            });
        }
        if out.iter().any(|existing| existing == text) {
            return Err(Error::Invalid {
                message: format!("mapping parameter '{key}' repeats the name {text:?}"),
            });
        }
        out.push(text.to_string());
    }
    if out.is_empty() {
        return Err(Error::Invalid {
            message: format!("mapping parameter '{key}' must not be empty"),
        });
    }
    Ok(out)
}

fn required_rounding(parameters: &Value) -> Result<Rounding> {
    match parameters.get("rounding").and_then(Value::as_str) {
        Some("exact") => Ok(Rounding::Exact),
        Some("floor") => Ok(Rounding::Floor),
        Some("ceil") => Ok(Rounding::Ceil),
        Some("round") => Ok(Rounding::Round),
        _ => Err(Error::Invalid {
            message: "unit_conversion rounding must be exact, floor, ceil, or round".into(),
        }),
    }
}

fn decode_map_policy(parameters: &Value) -> Result<(BTreeMap<String, Value>, UnmappedPolicy)> {
    let raw_map = parameters
        .get("map")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Invalid {
            message: "map object is required".into(),
        })?;
    let mut map = BTreeMap::new();
    for (k, v) in raw_map {
        map.insert(k.clone(), v.clone());
    }
    let unmapped = match parameters.get("unmapped") {
        None => UnmappedPolicy::Fail,
        Some(Value::String(s)) if s == "fail" => UnmappedPolicy::Fail,
        Some(Value::Object(obj)) => {
            if let Some(unknown) = obj.keys().find(|k| *k != "policy" && *k != "value") {
                return Err(Error::Invalid {
                    message: format!("unmapped policy has unknown field '{unknown}'"),
                });
            }
            if obj.get("policy").and_then(Value::as_str) != Some("const") {
                return Err(Error::Invalid {
                    message: "unmapped policy must be 'fail' or {policy: const, value}".into(),
                });
            }
            let value = obj.get("value").cloned().ok_or_else(|| Error::Invalid {
                message: "const unmapped policy requires value".into(),
            })?;
            UnmappedPolicy::Const(value)
        }
        Some(other) => {
            return Err(Error::Invalid {
                message: format!("unsupported unmapped policy {other}"),
            });
        }
    };
    Ok((map, unmapped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_injected_approval() {
        let value = json!({
            "schema_version": "1",
            "kind": "proposal",
            "approval": {"reviewer": "sneaky"},
            "id": "x",
            "source_contract": {"path": "a.yaml"},
            "destination_contract": {"path": "b.yaml"},
            "rules": []
        });
        assert!(matches!(
            parse_proposal_value(&value),
            Err(Error::InjectedApproval)
        ));
    }

    #[test]
    fn unknown_operation_is_explicit() {
        let err = decode_mapping("not_a_real_op", &json!({})).unwrap_err();
        assert!(matches!(err, Error::UnknownMappingOperation { .. }));
    }

    #[test]
    fn duplicate_split_targets_are_rejected_at_decode() {
        let err = decode_mapping(
            "split",
            &json!({"field": "name", "into": ["a", "a"], "separator": " "}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Invalid { .. }), "{err}");
        let err = decode_mapping(
            "merge",
            &json!({"fields": ["a", "a"], "into": "b", "separator": " "}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Invalid { .. }), "{err}");
    }

    #[test]
    fn unknown_cast_format_is_rejected_at_decode() {
        let err = decode_mapping(
            "safe_cast",
            &json!({"field": "x", "from": "string", "to": "string", "format": "uuid"}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Invalid { .. }), "{err}");
        assert!(decode_mapping(
            "safe_cast",
            &json!({"field": "x", "from": "string", "to": "string", "format": "email"}),
        )
        .is_ok());
    }
}
