use serde_json::{Map, Number, Value};

use crate::digest::{MAX_ARRAY, MAX_RULES};
use crate::error::{Error, Result};
use crate::proposal::{decode_mapping, MappingOp, Rounding, Rule, UnmappedPolicy};

/// Result of applying rules to one JSON object.
///
/// `lossiness` lists the events that actually happened for this input
/// (`default_inserted`, `enum_many_to_one`, `dropped`, `rounded`, `merged`).
/// It is computed from execution, not copied from the rule's declaration, so
/// a declared lossiness can be checked against observed behaviour.
#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub output: Value,
    pub lossiness: Vec<String>,
}

pub fn apply_rule(rule: &Rule, input: &Value) -> Result<ApplyResult> {
    let op = decode_mapping(&rule.operation, &rule.parameters)?;
    let mut output = input.clone();
    let obj = output.as_object_mut().ok_or_else(|| Error::Malformed {
        kind: "type".into(),
        field: rule.source.field.clone(),
        location: rule.id.clone(),
        message: "mapping rules in this subset apply to JSON objects".into(),
    })?;
    let mut lossiness = Vec::new();
    match op {
        MappingOp::Rename { from, to } => apply_rename(obj, &from, &to, &rule.id)?,
        MappingOp::Copy { from, to } => apply_copy(obj, &from, &to, &rule.id)?,
        MappingOp::Default { field, value } => {
            if apply_default(obj, &field, &value, &rule.id)? {
                lossiness.push("default_inserted".into());
            }
        }
        MappingOp::EnumMap {
            field,
            map,
            unmapped,
        } => {
            if apply_enum_map(obj, &field, &map, &unmapped, &rule.id)? {
                lossiness.push("enum_many_to_one".into());
            }
        }
        MappingOp::Drop { field } => {
            apply_drop(obj, &field, &rule.id)?;
            lossiness.push("dropped".into());
        }
        MappingOp::RequiredField { field } => apply_required(obj, &field, &rule.id)?,
        MappingOp::SafeCast {
            field,
            from,
            to,
            format,
        } => apply_safe_cast(obj, &field, &from, &to, format.as_deref(), &rule.id)?,
        MappingOp::UnitConvert {
            field,
            from_unit,
            to_unit,
            factor_num,
            factor_den,
            rounding,
        } => {
            if apply_unit(
                obj, &field, &from_unit, &to_unit, factor_num, factor_den, rounding, &rule.id,
            )? {
                lossiness.push("rounded".into());
            }
        }
        MappingOp::Split {
            field,
            into,
            separator,
        } => apply_split(obj, &field, &into, &separator, &rule.id)?,
        MappingOp::Merge {
            fields,
            into,
            separator,
        } => {
            apply_merge(obj, &fields, &into, &separator, &rule.id)?;
            lossiness.push("merged".into());
        }
        MappingOp::ArrayMap {
            field,
            map,
            unmapped,
        } => {
            if apply_array_map(obj, &field, &map, &unmapped, &rule.id)? {
                lossiness.push("enum_many_to_one".into());
            }
        }
        MappingOp::ArrayFilter { field, drop_null } => {
            if apply_array_filter(obj, &field, drop_null, &rule.id)? > 0 {
                lossiness.push("dropped".into());
            }
        }
        MappingOp::Pagination {
            from_items,
            from_token,
            to_data,
            to_cursor,
        } => apply_pagination(
            obj,
            &from_items,
            &from_token,
            &to_data,
            &to_cursor,
            &rule.id,
        )?,
    }
    Ok(ApplyResult { output, lossiness })
}

pub fn apply_rules(rules: &[Rule], input: &Value) -> Result<ApplyResult> {
    if rules.len() > MAX_RULES {
        return Err(Error::Bound {
            kind: "rules".into(),
            location: "apply".into(),
            message: format!("cannot apply more than {MAX_RULES} rules"),
        });
    }
    let mut current = input.clone();
    let mut lossiness = Vec::new();
    for rule in rules {
        let applied = apply_rule(rule, &current)?;
        current = applied.output;
        lossiness.extend(applied.lossiness);
    }
    Ok(ApplyResult {
        output: current,
        lossiness,
    })
}

fn apply_rename(obj: &mut Map<String, Value>, from: &str, to: &str, location: &str) -> Result<()> {
    if from == to {
        if !obj.contains_key(from) {
            return Err(Error::MissingField {
                field: from.into(),
                location: location.into(),
            });
        }
        return Ok(());
    }
    if obj.contains_key(to) {
        return Err(Error::Collision {
            field: to.into(),
            location: location.into(),
        });
    }
    let value = obj.remove(from).ok_or_else(|| Error::MissingField {
        field: from.into(),
        location: location.into(),
    })?;
    obj.insert(to.to_string(), value);
    Ok(())
}

fn apply_copy(obj: &mut Map<String, Value>, from: &str, to: &str, location: &str) -> Result<()> {
    if from == to {
        if !obj.contains_key(from) {
            return Err(Error::MissingField {
                field: from.into(),
                location: location.into(),
            });
        }
        return Ok(());
    }
    if obj.contains_key(to) {
        return Err(Error::Collision {
            field: to.into(),
            location: location.into(),
        });
    }
    let value = obj.get(from).cloned().ok_or_else(|| Error::MissingField {
        field: from.into(),
        location: location.into(),
    })?;
    obj.insert(to.to_string(), value);
    Ok(())
}

/// Returns true when the default was actually inserted.
fn apply_default(
    obj: &mut Map<String, Value>,
    field: &str,
    value: &Value,
    location: &str,
) -> Result<bool> {
    match obj.get(field) {
        None => {
            obj.insert(field.to_string(), value.clone());
            Ok(true)
        }
        Some(Value::Null) => Err(Error::ExplicitNull {
            field: field.into(),
            location: location.into(),
        }),
        Some(_) => Ok(false),
    }
}

fn apply_enum_map(
    obj: &mut Map<String, Value>,
    field: &str,
    map: &std::collections::BTreeMap<String, Value>,
    unmapped: &UnmappedPolicy,
    location: &str,
) -> Result<bool> {
    let current = obj.get(field).ok_or_else(|| Error::MissingField {
        field: field.into(),
        location: location.into(),
    })?;
    if current.is_null() {
        return Err(Error::ExplicitNull {
            field: field.into(),
            location: location.into(),
        });
    }
    let key = scalar_key(current, field, location)?;
    if let Some(mapped) = map.get(&key) {
        obj.insert(field.to_string(), mapped.clone());
        // Lossy only when this particular value shares its target with another key.
        let many_to_one = map.iter().any(|(k, v)| k != &key && v == mapped);
        return Ok(many_to_one);
    }
    match unmapped {
        UnmappedPolicy::Fail => Err(Error::UnknownEnum {
            field: field.into(),
            value: key,
            location: location.into(),
        }),
        UnmappedPolicy::Const(value) => {
            obj.insert(field.to_string(), value.clone());
            Ok(true)
        }
    }
}

fn apply_drop(obj: &mut Map<String, Value>, field: &str, location: &str) -> Result<()> {
    if obj.remove(field).is_none() {
        return Err(Error::MissingField {
            field: field.into(),
            location: location.into(),
        });
    }
    Ok(())
}

fn apply_required(obj: &mut Map<String, Value>, field: &str, location: &str) -> Result<()> {
    match obj.get(field) {
        None => Err(Error::MissingField {
            field: field.into(),
            location: location.into(),
        }),
        Some(Value::Null) => Err(Error::ExplicitNull {
            field: field.into(),
            location: location.into(),
        }),
        Some(_) => Ok(()),
    }
}

fn apply_safe_cast(
    obj: &mut Map<String, Value>,
    field: &str,
    from: &str,
    to: &str,
    format: Option<&str>,
    location: &str,
) -> Result<()> {
    let current = obj.get(field).cloned().ok_or_else(|| Error::MissingField {
        field: field.into(),
        location: location.into(),
    })?;
    if current.is_null() {
        return Err(Error::ExplicitNull {
            field: field.into(),
            location: location.into(),
        });
    }
    if !value_matches_type(&current, from) {
        return Err(Error::Malformed {
            kind: "type".into(),
            field: field.into(),
            location: location.into(),
            message: format!("safe_cast expected {from}"),
        });
    }
    let casted = match (from, to) {
        ("string", "integer") => {
            let text = current.as_str().unwrap();
            let parsed = text.parse::<i64>().map_err(|_| Error::Malformed {
                kind: "overflow".into(),
                field: field.into(),
                location: location.into(),
                message: "string does not parse as i64".into(),
            })?;
            Value::Number(parsed.into())
        }
        ("integer", "string") => Value::String(current.to_string().trim_matches('"').to_string()),
        ("string", "number") => {
            let text = current.as_str().unwrap();
            // Integers stay exact JSON integers at any i64/u64 size, never through f64.
            if let Ok(i) = text.parse::<i64>() {
                Value::Number(i.into())
            } else if let Ok(u) = text.parse::<u64>() {
                Value::Number(u.into())
            } else {
                let parsed = text.parse::<f64>().map_err(|_| Error::Malformed {
                    kind: "overflow".into(),
                    field: field.into(),
                    location: location.into(),
                    message: "string does not parse as number".into(),
                })?;
                if !parsed.is_finite() {
                    return Err(Error::Malformed {
                        kind: "overflow".into(),
                        field: field.into(),
                        location: location.into(),
                        message: "non-finite number".into(),
                    });
                }
                // The nearest double must denote the same decimal value as the string;
                // otherwise the cast would silently change the number.
                let exact = decimal_value(text)
                    .is_some_and(|v| decimal_value(&parsed.to_string()) == Some(v));
                if !exact {
                    return Err(Error::Malformed {
                    kind: "overflow".into(),
                    field: field.into(),
                    location: location.into(),
                    message: format!(
                        "string {text:?} has no exact JSON number; the nearest is {parsed}, so the cast would change the value"
                    ),
                });
                }
                Value::Number(Number::from_f64(parsed).ok_or_else(|| Error::Malformed {
                    kind: "overflow".into(),
                    field: field.into(),
                    location: location.into(),
                    message: "number cannot be encoded".into(),
                })?)
            }
        }
        ("number", "integer") => {
            let not_exact = || Error::Malformed {
                kind: "overflow".into(),
                field: field.into(),
                location: location.into(),
                message: "number is not an exact i64".into(),
            };
            // An integer-backed JSON number stays exact (never through f64, which rounds above
            // 2^53); one above i64 range is refused, not saturated.
            let exact = if let Some(i) = current.as_i64() {
                i
            } else if let Some(u) = current.as_u64() {
                i64::try_from(u).map_err(|_| not_exact())?
            } else {
                let n = current.as_f64().ok_or_else(|| Error::Malformed {
                    kind: "overflow".into(),
                    field: field.into(),
                    location: location.into(),
                    message: "number is not finite".into(),
                })?;
                // 2^63 itself is out of range: i64::MAX as f64 rounds up to it, so the upper
                // bound is exclusive at 2^63; -2^63 is representable and in range.
                const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;
                if !n.is_finite() || n.fract() != 0.0 || !(-TWO_POW_63..TWO_POW_63).contains(&n) {
                    return Err(not_exact());
                }
                n as i64
            };
            Value::Number(exact.into())
        }
        ("integer", "integer") | ("string", "string") | ("number", "number") => current,
        other => {
            return Err(Error::Unsupported {
                construct: "safe_cast".into(),
                location: location.into(),
                message: format!("cast {:?} is not implemented", other),
            });
        }
    };
    if let Some("email") = format {
        let text = casted.as_str().ok_or_else(|| Error::Malformed {
            kind: "format".into(),
            field: field.into(),
            location: location.into(),
            message: "email format requires a string".into(),
        })?;
        if !crate::schema::valid_email(text) {
            return Err(Error::Malformed {
                kind: "format".into(),
                field: field.into(),
                location: location.into(),
                message: "email format requires local@domain".into(),
            });
        }
    }
    obj.insert(field.to_string(), casted);
    Ok(())
}

/// Returns true when rounding changed the value (the conversion was inexact).
#[allow(clippy::too_many_arguments)]
fn apply_unit(
    obj: &mut Map<String, Value>,
    field: &str,
    from_unit: &str,
    to_unit: &str,
    factor_num: i64,
    factor_den: i64,
    rounding: Rounding,
    location: &str,
) -> Result<bool> {
    if from_unit.trim().is_empty() || to_unit.trim().is_empty() {
        return Err(Error::Invalid {
            message: "unit conversion requires non-empty from_unit and to_unit".into(),
        });
    }
    if factor_den == 0 {
        return Err(Error::Invalid {
            message: "unit conversion denominator cannot be zero".into(),
        });
    }
    let current = obj.get(field).cloned().ok_or_else(|| Error::MissingField {
        field: field.into(),
        location: location.into(),
    })?;
    if current.is_null() {
        return Err(Error::ExplicitNull {
            field: field.into(),
            location: location.into(),
        });
    }
    let n = current.as_i64().ok_or_else(|| Error::Malformed {
        kind: "type".into(),
        field: field.into(),
        location: location.into(),
        message: "unit_conversion requires an integer source value".into(),
    })?;
    let product = n.checked_mul(factor_num).ok_or_else(|| Error::Malformed {
        kind: "overflow".into(),
        field: field.into(),
        location: location.into(),
        message: "unit conversion overflowed i64 multiply".into(),
    })?;
    // Divide in i128 with the denominator's sign moved onto the numerator, so floor, ceil and the
    // round-half-up tie policy act on a positive denominator (a negative one inverted them), and
    // i64::MIN / -1 cannot panic: the result is range-checked into i64 as a typed error.
    let (mut p, mut d) = (product as i128, factor_den as i128);
    if d < 0 {
        p = -p;
        d = -d;
    }
    let inexact = p % d != 0;
    let converted = match rounding {
        Rounding::Exact => {
            if inexact {
                return Err(Error::Malformed {
                    kind: "overflow".into(),
                    field: field.into(),
                    location: location.into(),
                    message: "unit conversion is not exact for this value".into(),
                });
            }
            p / d
        }
        Rounding::Floor => p.div_euclid(d),
        Rounding::Ceil => p.div_euclid(d) + i128::from(inexact),
        Rounding::Round => p.div_euclid(d) + i128::from(p.rem_euclid(d) * 2 >= d),
    };
    let converted = i64::try_from(converted).map_err(|_| Error::Malformed {
        kind: "overflow".into(),
        field: field.into(),
        location: location.into(),
        message: "unit conversion result overflowed i64".into(),
    })?;
    obj.insert(field.to_string(), Value::Number(converted.into()));
    Ok(inexact)
}

fn apply_split(
    obj: &mut Map<String, Value>,
    field: &str,
    into: &[String],
    separator: &str,
    location: &str,
) -> Result<()> {
    if separator.is_empty() {
        return Err(Error::Invalid {
            message: "split separator must not be empty".into(),
        });
    }
    let current = obj.get(field).cloned().ok_or_else(|| Error::MissingField {
        field: field.into(),
        location: location.into(),
    })?;
    if current.is_null() {
        return Err(Error::ExplicitNull {
            field: field.into(),
            location: location.into(),
        });
    }
    let text = current.as_str().ok_or_else(|| Error::Malformed {
        kind: "type".into(),
        field: field.into(),
        location: location.into(),
        message: "split requires a string".into(),
    })?;
    let parts: Vec<&str> = text.splitn(into.len(), separator).collect();
    if parts.len() != into.len() {
        return Err(Error::Malformed {
            kind: "split".into(),
            field: field.into(),
            location: location.into(),
            message: format!("expected {} parts separated by {separator:?}", into.len()),
        });
    }
    for (i, name) in into.iter().enumerate() {
        if obj.contains_key(name) || into[..i].contains(name) {
            return Err(Error::Collision {
                field: name.clone(),
                location: location.into(),
            });
        }
    }
    obj.remove(field);
    for (name, part) in into.iter().zip(parts) {
        obj.insert(name.clone(), Value::String(part.to_string()));
    }
    Ok(())
}

fn apply_merge(
    obj: &mut Map<String, Value>,
    fields: &[String],
    into: &str,
    separator: &str,
    location: &str,
) -> Result<()> {
    if obj.contains_key(into) {
        return Err(Error::Collision {
            field: into.into(),
            location: location.into(),
        });
    }
    let mut parts = Vec::new();
    for field in fields {
        let value = obj.get(field).ok_or_else(|| Error::MissingField {
            field: field.clone(),
            location: location.into(),
        })?;
        if value.is_null() {
            return Err(Error::ExplicitNull {
                field: field.clone(),
                location: location.into(),
            });
        }
        let text = value.as_str().ok_or_else(|| Error::Malformed {
            kind: "type".into(),
            field: field.clone(),
            location: location.into(),
            message: "merge requires string fields".into(),
        })?;
        parts.push(text.to_string());
    }
    for field in fields {
        obj.remove(field);
    }
    obj.insert(into.to_string(), Value::String(parts.join(separator)));
    Ok(())
}

fn apply_array_map(
    obj: &mut Map<String, Value>,
    field: &str,
    map: &std::collections::BTreeMap<String, Value>,
    unmapped: &UnmappedPolicy,
    location: &str,
) -> Result<bool> {
    let current = obj.get(field).cloned().ok_or_else(|| Error::MissingField {
        field: field.into(),
        location: location.into(),
    })?;
    if current.is_null() {
        return Err(Error::ExplicitNull {
            field: field.into(),
            location: location.into(),
        });
    }
    let items = current.as_array().ok_or_else(|| Error::Malformed {
        kind: "type".into(),
        field: field.into(),
        location: location.into(),
        message: "array_map requires an array".into(),
    })?;
    if items.len() > MAX_ARRAY {
        return Err(Error::Bound {
            kind: "array".into(),
            location: location.into(),
            message: format!("array longer than {MAX_ARRAY} items"),
        });
    }
    let mut mapped = Vec::new();
    let mut lossy = false;
    for (i, item) in items.iter().enumerate() {
        // Each item is handled as enum_map handles a field: null is explicit_null.
        if item.is_null() {
            return Err(Error::ExplicitNull {
                field: field.into(),
                location: format!("{location}/{i}"),
            });
        }
        let key = scalar_key(item, field, &format!("{location}/{i}"))?;
        if let Some(value) = map.get(&key) {
            mapped.push(value.clone());
        } else {
            match unmapped {
                UnmappedPolicy::Fail => {
                    return Err(Error::UnknownEnum {
                        field: field.into(),
                        value: key,
                        location: format!("{location}/{i}"),
                    });
                }
                UnmappedPolicy::Const(value) => {
                    mapped.push(value.clone());
                    lossy = true;
                }
            }
        }
    }
    obj.insert(field.to_string(), Value::Array(mapped));
    Ok(lossy)
}

/// Returns how many items were removed.
fn apply_array_filter(
    obj: &mut Map<String, Value>,
    field: &str,
    drop_null: bool,
    location: &str,
) -> Result<usize> {
    let current = obj.get(field).cloned().ok_or_else(|| Error::MissingField {
        field: field.into(),
        location: location.into(),
    })?;
    if current.is_null() {
        return Err(Error::ExplicitNull {
            field: field.into(),
            location: location.into(),
        });
    }
    let items = current.as_array().ok_or_else(|| Error::Malformed {
        kind: "type".into(),
        field: field.into(),
        location: location.into(),
        message: "array_filter requires an array".into(),
    })?;
    if items.len() > MAX_ARRAY {
        return Err(Error::Bound {
            kind: "array".into(),
            location: location.into(),
            message: format!("array longer than {MAX_ARRAY} items"),
        });
    }
    let kept: Vec<Value> = if drop_null {
        items
            .iter()
            .filter(|item| !item.is_null())
            .cloned()
            .collect()
    } else {
        items.clone()
    };
    let removed = items.len() - kept.len();
    obj.insert(field.to_string(), Value::Array(kept));
    Ok(removed)
}

fn apply_pagination(
    obj: &mut Map<String, Value>,
    from_items: &str,
    from_token: &str,
    to_data: &str,
    to_cursor: &str,
    location: &str,
) -> Result<()> {
    apply_rename(obj, from_items, to_data, location)?;
    if from_token == to_cursor {
        if !obj.contains_key(from_token) {
            return Err(Error::MissingField {
                field: from_token.into(),
                location: location.into(),
            });
        }
        return Ok(());
    }
    apply_rename(obj, from_token, to_cursor, location)
}

fn value_matches_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        _ => false,
    }
}

fn scalar_key(value: &Value, field: &str, location: &str) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(Error::Malformed {
            kind: "type".into(),
            field: field.into(),
            location: location.into(),
            message: "enum_map requires a scalar value".into(),
        }),
    }
}

/// The exact value a decimal number string denotes, as (negative, significant
/// digits without leading or trailing zeros, power of ten): "2.50", "25e-1"
/// and "0.25e1" all give (false, "25", -1). Zero is (false, "", 0). `None` for
/// anything that is not a plain decimal (NaN, infinity, hex, empty).
fn decimal_value(text: &str) -> Option<(bool, String, i64)> {
    let (negative, rest) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let (mantissa, exponent) = match rest.find(['e', 'E']) {
        Some(i) => (&rest[..i], rest[i + 1..].parse::<i64>().ok()?),
        None => (rest, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (int.is_empty() && frac.is_empty())
        || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{int}{frac}");
    let significant = digits.trim_end_matches('0');
    let exponent = exponent
        .checked_sub(i64::try_from(frac.len()).ok()?)?
        .checked_add(i64::try_from(digits.len() - significant.len()).ok()?)?;
    let significant = significant.trim_start_matches('0');
    if significant.is_empty() {
        return Some((false, String::new(), 0));
    }
    Some((negative, significant.to_string(), exponent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::OpIdentity;
    use crate::proposal::{Rule, Target};
    use serde_json::json;

    fn target(field: &str) -> Target {
        Target {
            operation: OpIdentity {
                method: "POST".into(),
                path: "/x".into(),
                operation_id: Some("x".into()),
            },
            location: "request".into(),
            response_status: None,
            field: field.into(),
        }
    }

    fn rule(op: &str, params: Value) -> Rule {
        Rule {
            id: "r1".into(),
            operation: op.into(),
            source: target("a"),
            destination: target("b"),
            parameters: params,
            preconditions: vec![],
            postconditions: vec![],
            evidence: "test".into(),
            lossiness: "none".into(),
            reversibility: "reversible".into(),
            fixture_refs: vec![],
        }
    }

    #[test]
    fn rename_does_not_overwrite() {
        let err = apply_rule(
            &rule("rename", json!({"from":"id","to":"user_id"})),
            &json!({"id":"1","user_id":"2"}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Collision { .. }));
    }

    #[test]
    fn copy_keeps_source_and_rejects_collision() {
        let out = apply_rule(
            &rule("copy", json!({"from":"name","to":"display_name"})),
            &json!({"name":"Ada"}),
        )
        .unwrap();
        assert_eq!(out.output["name"], "Ada");
        assert_eq!(out.output["display_name"], "Ada");
        let err = apply_rule(
            &rule("copy", json!({"from":"name","to":"display_name"})),
            &json!({"name":"Ada","display_name":"other"}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Collision { .. }));
    }

    #[test]
    fn default_does_not_replace_null() {
        let err = apply_rule(
            &rule("default", json!({"field":"status","value":"active"})),
            &json!({"name":"w","status":null}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::ExplicitNull { .. }));
    }

    #[test]
    fn default_fills_missing() {
        let out = apply_rule(
            &rule("default", json!({"field":"status","value":"active"})),
            &json!({"name":"w"}),
        )
        .unwrap();
        assert_eq!(out.output["status"], "active");
        assert_eq!(out.lossiness, vec!["default_inserted".to_string()]);
    }

    #[test]
    fn default_present_reports_no_lossiness() {
        let out = apply_rule(
            &rule("default", json!({"field":"status","value":"active"})),
            &json!({"name":"w","status":"archived"}),
        )
        .unwrap();
        assert_eq!(out.output["status"], "archived");
        assert!(out.lossiness.is_empty(), "{:?}", out.lossiness);
    }

    #[test]
    fn lossiness_is_computed_not_declared() {
        let mut declared = rule("rename", json!({"from":"id","to":"user_id"}));
        declared.lossiness = "dropped".into();
        let out = apply_rule(&declared, &json!({"id":"1"})).unwrap();
        assert!(out.lossiness.is_empty(), "{:?}", out.lossiness);
        let out = apply_rule(
            &rule("drop", json!({"field":"legacy"})),
            &json!({"legacy":true}),
        )
        .unwrap();
        assert_eq!(out.lossiness, vec!["dropped".to_string()]);
        let out = apply_rule(
            &rule("array_filter", json!({"field":"tags","drop_null":true})),
            &json!({"tags":["a","b"]}),
        )
        .unwrap();
        assert!(out.lossiness.is_empty());
        let out = apply_rule(
            &rule("array_filter", json!({"field":"tags","drop_null":true})),
            &json!({"tags":["a",null]}),
        )
        .unwrap();
        assert_eq!(out.lossiness, vec!["dropped".to_string()]);
    }

    #[test]
    fn enum_many_to_one_is_reported_only_for_collapsed_values() {
        let r = rule(
            "enum_map",
            json!({"field":"c","map":{"A":"x","B":"x","C":"y"},"unmapped":"fail"}),
        );
        assert_eq!(
            apply_rule(&r, &json!({"c":"A"})).unwrap().lossiness,
            vec!["enum_many_to_one".to_string()]
        );
        assert!(apply_rule(&r, &json!({"c":"C"}))
            .unwrap()
            .lossiness
            .is_empty());
    }

    #[test]
    fn split_duplicate_targets_collide() {
        // decode_mapping rejects duplicates first; the interpreter guards too.
        let mut obj = serde_json::Map::new();
        obj.insert("n".to_string(), json!("x y"));
        let err = apply_split(
            &mut obj,
            "n",
            &["a".to_string(), "a".to_string()],
            " ",
            "r1",
        )
        .unwrap_err();
        assert!(matches!(err, Error::Collision { .. }));
        assert_eq!(obj.get("n"), Some(&json!("x y")), "input must be untouched");
    }

    #[test]
    fn unit_rounding_reports_rounded_only_when_inexact() {
        let r = rule(
            "unit_conversion",
            json!({"field":"m","from_unit":"g","to_unit":"kg","factor_num":1,"factor_den":1000,"rounding":"round"}),
        );
        let exact = apply_rule(&r, &json!({"m": 2000})).unwrap();
        assert_eq!(exact.output["m"], 2);
        assert!(exact.lossiness.is_empty());
        let inexact = apply_rule(&r, &json!({"m": 1500})).unwrap();
        assert_eq!(inexact.output["m"], 2);
        assert_eq!(inexact.lossiness, vec!["rounded".to_string()]);
    }

    #[test]
    fn unknown_enum_fails() {
        let err = apply_rule(
            &rule(
                "enum_map",
                json!({"field":"color","map":{"RED":"red"},"unmapped":"fail"}),
            ),
            &json!({"color":"YELLOW"}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::UnknownEnum { .. }));
    }

    #[test]
    fn drop_removes_field_and_missing_fails() {
        let out = apply_rule(
            &rule("drop", json!({"field": "legacy"})),
            &json!({"legacy": true, "name": "a"}),
        )
        .unwrap();
        assert_eq!(out.output, json!({"name": "a"}));
        let err = apply_rule(
            &rule("drop", json!({"field": "legacy"})),
            &json!({"name": "a"}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::MissingField { .. }));
    }

    #[test]
    fn required_field_rejects_null() {
        let err = apply_rule(
            &rule("required_field", json!({"field": "email"})),
            &json!({"email": null}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::ExplicitNull { .. }));
    }

    #[test]
    fn safe_cast_overflow_fails() {
        let err = apply_rule(
            &rule(
                "safe_cast",
                json!({"field": "count", "from": "string", "to": "integer"}),
            ),
            &json!({"count": "999999999999999999999"}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }));
    }

    #[test]
    fn unit_exact_and_inexact() {
        let out = apply_rule(
            &rule(
                "unit_conversion",
                json!({
                    "field": "mass",
                    "from_unit": "kg",
                    "to_unit": "g",
                    "factor_num": 1000,
                    "factor_den": 1,
                    "rounding": "exact"
                }),
            ),
            &json!({"mass": 2}),
        )
        .unwrap();
        assert_eq!(out.output["mass"], 2000);
        let err = apply_rule(
            &rule(
                "unit_conversion",
                json!({
                    "field": "mass",
                    "from_unit": "g",
                    "to_unit": "kg",
                    "factor_num": 1,
                    "factor_den": 1000,
                    "rounding": "exact"
                }),
            ),
            &json!({"mass": 1500}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }));
    }
}
