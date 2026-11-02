//! Static type-checking of a proposal against the source and destination
//! contracts. Every check here runs before approval; an ill-typed rule set
//! never becomes an executable migration.
//!
//! Diagnostics carry a severity. `error` blocks approval. `warning` is
//! surfaced in the review document but does not block.

use serde_json::Value;

use crate::error::{Error, Result};
use crate::parse::{operations, OpenApiDoc, OperationView};
use crate::proposal::{decode_mapping, MappingOp, Proposal, Rule, Target, UnmappedPolicy};
use crate::schema::{AdditionalProperties, ResolvedSchema};

pub const LOSSINESS_VOCABULARY: &[&str] = &[
    "none",
    "dropped",
    "default_inserted",
    "enum_many_to_one",
    "rounded",
    "merged",
];

pub const REVERSIBILITY_VOCABULARY: &[&str] =
    &["reversible", "irreversible", "partially_reversible"];

const SUPPORTED_CASTS: &[(&str, &str)] = &[
    ("string", "integer"),
    ("integer", "string"),
    ("string", "number"),
    ("number", "integer"),
    ("integer", "integer"),
    ("string", "string"),
    ("number", "number"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub rule_id: String,
    pub severity: Severity,
    pub location: String,
    pub message: String,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Type-check every rule and return all diagnostics (errors and warnings).
/// This never fails on a typing problem; it only fails when the proposal or
/// contracts cannot be inspected at all.
pub fn check(
    proposal: &Proposal,
    source: &OpenApiDoc,
    dest: &OpenApiDoc,
) -> Result<Vec<Diagnostic>> {
    let source_ops = operations(source)?;
    let dest_ops = operations(dest)?;
    let mut diagnostics = Vec::new();
    for rule in &proposal.rules {
        let mut sink = Sink::new(&rule.id);
        check_rule(rule, &source_ops, &dest_ops, &mut sink)?;
        diagnostics.extend(sink.notes);
    }
    Ok(diagnostics)
}

/// Type-check and fail when any error-severity diagnostic exists. Warnings are
/// returned to the caller. This is the gate used by `approve`.
pub fn typecheck_proposal(
    proposal: &Proposal,
    source: &OpenApiDoc,
    dest: &OpenApiDoc,
) -> Result<Vec<Diagnostic>> {
    let diagnostics = check(proposal, source, dest)?;
    let errors: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.is_error()).collect();
    if !errors.is_empty() {
        return Err(Error::Invalid {
            message: format!(
                "type-check failed with {} error(s): {}",
                errors.len(),
                errors
                    .iter()
                    .map(|d| format!("{} [{}]: {}", d.rule_id, d.location, d.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        });
    }
    Ok(diagnostics)
}

struct Sink<'a> {
    rule_id: &'a str,
    notes: Vec<Diagnostic>,
}

impl<'a> Sink<'a> {
    fn new(rule_id: &'a str) -> Self {
        Self {
            rule_id,
            notes: Vec::new(),
        }
    }

    fn error(&mut self, location: impl Into<String>, message: impl Into<String>) {
        self.notes.push(Diagnostic {
            rule_id: self.rule_id.to_string(),
            severity: Severity::Error,
            location: location.into(),
            message: message.into(),
        });
    }

    fn warning(&mut self, location: impl Into<String>, message: impl Into<String>) {
        self.notes.push(Diagnostic {
            rule_id: self.rule_id.to_string(),
            severity: Severity::Warning,
            location: location.into(),
            message: message.into(),
        });
    }
}

fn check_rule(
    rule: &Rule,
    source_ops: &[OperationView],
    dest_ops: &[OperationView],
    sink: &mut Sink,
) -> Result<()> {
    check_vocabulary(rule, sink);
    let mapping = decode_mapping(&rule.operation, &rule.parameters)?;
    let source_schema = body_schema(source_ops, &rule.source, "source", sink);
    let dest_schema = body_schema(dest_ops, &rule.destination, "destination", sink);
    let (Some(src), Some(dst)) = (source_schema, dest_schema) else {
        return Ok(());
    };
    match mapping {
        MappingOp::Rename { from, to } => {
            check_target_field(&rule.source, &from, "source", sink);
            check_target_field(&rule.destination, &to, "destination", sink);
            let Some(s) = require_property(src, &from, "source", sink) else {
                return Ok(());
            };
            let Some(d) = require_property(dst, &to, "destination", sink) else {
                return Ok(());
            };
            if from != to && dst.required.iter().any(|r| r == &from) {
                sink.error(
                    format!("destination/{from}"),
                    format!("destination still requires {from}, which rename removes"),
                );
            }
            if from != to && src.properties.contains_key(&to) {
                sink.warning(
                    format!("source/{to}"),
                    format!("source already declares {to}; inputs carrying it collide at runtime"),
                );
            }
            check_value_flow(s, d, &from, &to, sink);
        }
        MappingOp::Copy { from, to } => {
            check_target_field(&rule.source, &from, "source", sink);
            check_target_field(&rule.destination, &to, "destination", sink);
            let Some(s) = require_property(src, &from, "source", sink) else {
                return Ok(());
            };
            let Some(d) = require_property(dst, &to, "destination", sink) else {
                return Ok(());
            };
            if from != to && !dst.properties.contains_key(&from) {
                if dst.additional_properties == AdditionalProperties::Forbidden {
                    sink.error(
                        format!("destination/{from}"),
                        format!(
                            "copy keeps {from}, but the destination forbids undeclared properties"
                        ),
                    );
                } else {
                    sink.warning(
                        format!("destination/{from}"),
                        format!("copy keeps {from}, which the destination does not declare"),
                    );
                }
            }
            if from != to && src.properties.contains_key(&to) {
                sink.warning(
                    format!("source/{to}"),
                    format!("source already declares {to}; inputs carrying it collide at runtime"),
                );
            }
            check_value_flow(s, d, &from, &to, sink);
        }
        MappingOp::Default { field, value } => {
            check_target_field(&rule.destination, &field, "destination", sink);
            let Some(d) = require_property(dst, &field, "destination", sink) else {
                return Ok(());
            };
            if src.required.iter().any(|r| r == &field) {
                sink.warning(
                    format!("source/{field}"),
                    format!("source already requires {field}; the default is never inserted"),
                );
            }
            if let Some(s) = src.properties.get(&field) {
                check_value_flow(s, d, &field, &field, sink);
            }
            if let Err(err) =
                crate::schema::validate_instance(d, &value, &format!("destination/{field}"))
            {
                sink.error(
                    format!("destination/{field}"),
                    format!("default value {value} does not satisfy the destination schema: {err}"),
                );
            }
        }
        MappingOp::EnumMap {
            field,
            map,
            unmapped,
        } => {
            check_target_field(&rule.source, &field, "source", sink);
            check_target_field(&rule.destination, &field, "destination", sink);
            let Some(s) = require_property(src, &field, "source", sink) else {
                return Ok(());
            };
            let Some(d) = require_property(dst, &field, "destination", sink) else {
                return Ok(());
            };
            let declared = rule.parameters.get("unmapped").is_some();
            check_enum_mapping(
                s,
                d,
                &field,
                &map,
                (&unmapped, declared),
                &rule.lossiness,
                sink,
            );
            check_nullability(s, d, &field, sink);
        }
        MappingOp::Drop { field } => {
            check_target_field(&rule.source, &field, "source", sink);
            if require_property(src, &field, "source", sink).is_none() {
                return Ok(());
            }
            if dst.required.iter().any(|r| r == &field) {
                sink.error(
                    format!("destination/{field}"),
                    format!("drop target {field} is still required on the destination"),
                );
            } else if dst.properties.contains_key(&field) {
                sink.warning(
                    format!("destination/{field}"),
                    format!("destination still declares dropped field {field}"),
                );
            }
            if rule.lossiness == "none" {
                sink.warning(
                    "lossiness",
                    "drop discards a value but declares lossiness none",
                );
            }
        }
        MappingOp::RequiredField { field } => {
            check_target_field(&rule.destination, &field, "destination", sink);
            let in_source = src.properties.contains_key(&field);
            let in_dest = dst.properties.contains_key(&field);
            if !in_source && !in_dest {
                sink.error(
                    format!("destination/{field}"),
                    format!("neither contract declares property {field}"),
                );
                return Ok(());
            }
            if !in_dest {
                sink.warning(
                    format!("destination/{field}"),
                    format!("destination does not declare required field {field}"),
                );
            }
            if !dst.required.iter().any(|r| r == &field) {
                sink.warning(
                    format!("destination/{field}"),
                    format!("destination schema does not require {field}; the rule is stricter than the contract"),
                );
            }
        }
        MappingOp::SafeCast {
            field,
            from,
            to,
            format,
        } => {
            check_target_field(&rule.source, &field, "source", sink);
            check_target_field(&rule.destination, &field, "destination", sink);
            let Some(s) = require_property(src, &field, "source", sink) else {
                return Ok(());
            };
            let Some(d) = require_property(dst, &field, "destination", sink) else {
                return Ok(());
            };
            if !SUPPORTED_CASTS.contains(&(from.as_str(), to.as_str())) {
                sink.error(
                    "parameters",
                    format!("safe_cast {from} -> {to} is not an implemented cast"),
                );
            }
            if let Some(types) = declared_types(s) {
                if !types.iter().any(|t| t == &from) {
                    sink.error(
                        format!("source/{field}"),
                        format!(
                            "source declares {field} as {} but the cast reads {from}",
                            types.join("|")
                        ),
                    );
                }
            }
            if let Some(types) = declared_types(d) {
                if !types
                    .iter()
                    .any(|t| t == &to || (t == "number" && to == "integer"))
                {
                    sink.error(
                        format!("destination/{field}"),
                        format!(
                            "destination declares {field} as {} but the cast writes {to}",
                            types.join("|")
                        ),
                    );
                }
            }
            if let Some(fmt) = format {
                if fmt != "email" {
                    sink.error(
                        "parameters",
                        format!("safe_cast format {fmt} is not implemented"),
                    );
                } else if to != "string" {
                    sink.error(
                        "parameters",
                        "email format validation requires a string result",
                    );
                }
            }
            check_nullability(s, d, &field, sink);
        }
        MappingOp::UnitConvert {
            field,
            from_unit,
            to_unit,
            factor_num,
            factor_den,
            rounding,
        } => {
            check_target_field(&rule.source, &field, "source", sink);
            check_target_field(&rule.destination, &field, "destination", sink);
            let Some(s) = require_property(src, &field, "source", sink) else {
                return Ok(());
            };
            let Some(d) = require_property(dst, &field, "destination", sink) else {
                return Ok(());
            };
            if let Some(types) = declared_types(s) {
                if !types.iter().any(|t| t == "integer") {
                    sink.error(
                        format!("source/{field}"),
                        format!(
                            "unit_conversion reads integers but the source declares {}",
                            types.join("|")
                        ),
                    );
                }
            }
            if let Some(types) = declared_types(d) {
                if !types.iter().any(|t| t == "integer" || t == "number") {
                    sink.error(
                        format!("destination/{field}"),
                        format!(
                            "unit_conversion writes integers but the destination declares {}",
                            types.join("|")
                        ),
                    );
                }
            }
            if factor_den == 0 {
                sink.error("parameters", "factor_den must not be zero");
            }
            if factor_num == 0 {
                sink.warning(
                    "parameters",
                    "factor_num is zero; every value converts to 0",
                );
            }
            if from_unit == to_unit {
                sink.warning(
                    "parameters",
                    format!("from_unit and to_unit are both {from_unit}"),
                );
            }
            if rounding != crate::proposal::Rounding::Exact && rule.lossiness == "none" {
                sink.warning(
                    "lossiness",
                    "non-exact rounding can discard precision but the rule declares lossiness none",
                );
            }
            check_nullability(s, d, &field, sink);
        }
        MappingOp::Split {
            field,
            into,
            separator,
        } => {
            check_target_field(&rule.source, &field, "source", sink);
            if separator.is_empty() {
                sink.error("parameters", "split separator must not be empty");
            }
            let Some(s) = require_property(src, &field, "source", sink) else {
                return Ok(());
            };
            require_type(s, "string", &format!("source/{field}"), "split reads", sink);
            if into.iter().any(|name| name == &field) {
                sink.error(
                    "parameters",
                    format!("split target {field} equals the source field and always collides"),
                );
            }
            if dst.required.iter().any(|r| r == &field) {
                sink.error(
                    format!("destination/{field}"),
                    format!("destination still requires {field}, which split removes"),
                );
            }
            for name in &into {
                if let Some(d) = require_property(dst, name, "destination", sink) {
                    require_type(
                        d,
                        "string",
                        &format!("destination/{name}"),
                        "split writes",
                        sink,
                    );
                }
            }
        }
        MappingOp::Merge {
            fields,
            into,
            separator,
        } => {
            check_target_field(&rule.destination, &into, "destination", sink);
            if fields.iter().any(|name| name == &into) {
                sink.error(
                    "parameters",
                    format!(
                        "merge target {into} is also a merged source field and always collides"
                    ),
                );
            }
            let _ = separator;
            for name in &fields {
                if let Some(s) = require_property(src, name, "source", sink) {
                    require_type(s, "string", &format!("source/{name}"), "merge reads", sink);
                }
                if dst.required.iter().any(|r| r == name) {
                    sink.error(
                        format!("destination/{name}"),
                        format!("destination still requires {name}, which merge removes"),
                    );
                }
            }
            if let Some(d) = require_property(dst, &into, "destination", sink) {
                require_type(
                    d,
                    "string",
                    &format!("destination/{into}"),
                    "merge writes",
                    sink,
                );
            }
            if rule.lossiness == "none" {
                sink.warning(
                    "lossiness",
                    "merge collapses several fields into one but declares lossiness none",
                );
            }
        }
        MappingOp::ArrayMap {
            field,
            map,
            unmapped,
        } => {
            check_target_field(&rule.source, &field, "source", sink);
            check_target_field(&rule.destination, &field, "destination", sink);
            let Some(s) = require_property(src, &field, "source", sink) else {
                return Ok(());
            };
            let Some(d) = require_property(dst, &field, "destination", sink) else {
                return Ok(());
            };
            require_type(
                s,
                "array",
                &format!("source/{field}"),
                "array_map reads",
                sink,
            );
            require_type(
                d,
                "array",
                &format!("destination/{field}"),
                "array_map writes",
                sink,
            );
            if let (Some(si), Some(di)) = (&s.items, &d.items) {
                check_enum_mapping(
                    si,
                    di,
                    &format!("{field}/items"),
                    &map,
                    (&unmapped, rule.parameters.get("unmapped").is_some()),
                    &rule.lossiness,
                    sink,
                );
            }
        }
        MappingOp::ArrayFilter { field, drop_null } => {
            check_target_field(&rule.source, &field, "source", sink);
            check_target_field(&rule.destination, &field, "destination", sink);
            let Some(s) = require_property(src, &field, "source", sink) else {
                return Ok(());
            };
            let Some(d) = require_property(dst, &field, "destination", sink) else {
                return Ok(());
            };
            require_type(
                s,
                "array",
                &format!("source/{field}"),
                "array_filter reads",
                sink,
            );
            require_type(
                d,
                "array",
                &format!("destination/{field}"),
                "array_filter writes",
                sink,
            );
            if !drop_null {
                sink.warning(
                    "parameters",
                    "array_filter with drop_null=false changes nothing",
                );
            } else if let Some(items) = &s.items {
                if !items.nullable && rule.lossiness == "none" {
                    sink.warning(
                        format!("source/{field}/items"),
                        "source items are not nullable; drop_null never removes anything",
                    );
                }
            }
            if drop_null && rule.lossiness == "none" {
                sink.warning(
                    "lossiness",
                    "array_filter can discard items but declares lossiness none",
                );
            }
        }
        MappingOp::Pagination {
            from_items,
            from_token,
            to_data,
            to_cursor,
        } => {
            check_target_field(&rule.source, &from_items, "source", sink);
            check_target_field(&rule.destination, &to_data, "destination", sink);
            let items = require_property(src, &from_items, "source", sink);
            let token = require_property(src, &from_token, "source", sink);
            let data = require_property(dst, &to_data, "destination", sink);
            let cursor = require_property(dst, &to_cursor, "destination", sink);
            if let Some(items) = items {
                require_type(
                    items,
                    "array",
                    &format!("source/{from_items}"),
                    "pagination reads",
                    sink,
                );
                if let Some(data) = data {
                    require_type(
                        data,
                        "array",
                        &format!("destination/{to_data}"),
                        "pagination writes",
                        sink,
                    );
                    check_value_flow(items, data, &from_items, &to_data, sink);
                }
            }
            if let (Some(token), Some(cursor)) = (token, cursor) {
                check_value_flow(token, cursor, &from_token, &to_cursor, sink);
            }
            for moved in [&from_items, &from_token] {
                let target = if moved == &from_items {
                    &to_data
                } else {
                    &to_cursor
                };
                if moved != target && dst.required.iter().any(|r| r == moved) {
                    sink.error(
                        format!("destination/{moved}"),
                        format!("destination still requires {moved}, which pagination renames"),
                    );
                }
            }
        }
    }
    Ok(())
}

fn check_vocabulary(rule: &Rule, sink: &mut Sink) {
    if !LOSSINESS_VOCABULARY.contains(&rule.lossiness.as_str()) {
        sink.error(
            "lossiness",
            format!(
                "lossiness {:?} is not in the vocabulary {}",
                rule.lossiness,
                LOSSINESS_VOCABULARY.join("|")
            ),
        );
    }
    if !REVERSIBILITY_VOCABULARY.contains(&rule.reversibility.as_str()) {
        sink.error(
            "reversibility",
            format!(
                "reversibility {:?} is not in the vocabulary {}",
                rule.reversibility,
                REVERSIBILITY_VOCABULARY.join("|")
            ),
        );
    }
}

fn check_target_field(target: &Target, expected: &str, side: &str, sink: &mut Sink) {
    if target.field != expected {
        sink.warning(
            format!("{side}/field"),
            format!(
                "{side}.field is {:?} but the parameters name {expected:?}",
                target.field
            ),
        );
    }
}

fn require_property<'s>(
    schema: &'s ResolvedSchema,
    field: &str,
    side: &str,
    sink: &mut Sink,
) -> Option<&'s ResolvedSchema> {
    match schema.properties.get(field) {
        Some(prop) => Some(prop),
        None => {
            sink.error(
                format!("{side}/{field}"),
                format!("{side} schema does not declare property {field}"),
            );
            None
        }
    }
}

fn require_type(
    schema: &ResolvedSchema,
    expected: &str,
    location: &str,
    verb: &str,
    sink: &mut Sink,
) {
    if let Some(types) = declared_types(schema) {
        if !types.iter().any(|t| t == expected) {
            sink.error(
                location,
                format!(
                    "{verb} {expected} values but the schema declares {}",
                    types.join("|")
                ),
            );
        }
    }
}

/// Non-null declared types, or `None` when the schema does not declare a type
/// at all (treated as "any" so that untyped properties are not falsely rejected).
fn declared_types(schema: &ResolvedSchema) -> Option<Vec<String>> {
    schema.original.get("type")?;
    Some(
        schema
            .types
            .iter()
            .filter(|t| t.as_str() != "null")
            .cloned()
            .collect(),
    )
}

/// A value read from `s` is written unchanged into `d`. Types, nullability and
/// enum membership must line up.
fn check_value_flow(s: &ResolvedSchema, d: &ResolvedSchema, from: &str, to: &str, sink: &mut Sink) {
    if let (Some(st), Some(dt)) = (declared_types(s), declared_types(d)) {
        let compatible = st
            .iter()
            .all(|t| dt.contains(t) || (t == "integer" && dt.iter().any(|x| x == "number")));
        if !compatible {
            sink.error(
                format!("destination/{to}"),
                format!(
                    "{from} is {} on the source but {to} is {} on the destination",
                    st.join("|"),
                    dt.join("|")
                ),
            );
        }
    }
    check_nullability(s, d, to, sink);
    match (&s.enum_values, &d.enum_values) {
        (Some(se), Some(de)) => {
            let missing: Vec<String> = se
                .iter()
                .filter(|v| !de.contains(v))
                .map(Value::to_string)
                .collect();
            if !missing.is_empty() {
                sink.error(
                    format!("destination/{to}"),
                    format!(
                        "source enum values {} are not accepted by the destination enum",
                        missing.join(", ")
                    ),
                );
            }
        }
        (None, Some(_)) => sink.warning(
            format!("destination/{to}"),
            format!("{from} is unconstrained on the source but {to} is an enum on the destination"),
        ),
        _ => {}
    }
}

fn check_nullability(s: &ResolvedSchema, d: &ResolvedSchema, field: &str, sink: &mut Sink) {
    if s.nullable && !d.nullable {
        sink.warning(
            format!("destination/{field}"),
            format!(
                "{field} is nullable on the source but not on the destination; null inputs fail"
            ),
        );
    }
}

fn check_enum_mapping(
    s: &ResolvedSchema,
    d: &ResolvedSchema,
    field: &str,
    map: &std::collections::BTreeMap<String, Value>,
    (unmapped, policy_declared): (&UnmappedPolicy, bool),
    declared_lossiness: &str,
    sink: &mut Sink,
) {
    if let Some(source_enum) = &s.enum_values {
        for key in map.keys() {
            let matches = source_enum
                .iter()
                .filter(|v| enum_key_matches(v, key))
                .count();
            if matches == 0 {
                sink.error(
                    format!("source/{field}"),
                    format!("map key {key:?} is not a source enum value"),
                );
            } else if matches > 1 {
                // Keys are text: "1" cannot tell the number 1 from the string "1".
                sink.error(
                    format!("source/{field}"),
                    format!(
                        "map key {key:?} matches more than one source enum value (for example the number and the string with that text); the map cannot tell them apart"
                    ),
                );
            }
        }
        if matches!(unmapped, UnmappedPolicy::Fail) {
            let unmapped_values: Vec<String> = source_enum
                .iter()
                .filter(|v| !v.is_null())
                .filter(|v| !map.keys().any(|k| enum_key_matches(v, k)))
                .map(Value::to_string)
                .collect();
            if !unmapped_values.is_empty() && !policy_declared {
                sink.error(
                    format!("source/{field}"),
                    format!(
                        "source enum values {} have no mapping and the rule sets no unmapped policy; cover them or set unmapped (\"fail\" or a const value)",
                        unmapped_values.join(", ")
                    ),
                );
            } else if !unmapped_values.is_empty() {
                sink.warning(
                    format!("source/{field}"),
                    format!(
                        "source enum values {} have no mapping; such inputs fail with unknown_enum",
                        unmapped_values.join(", ")
                    ),
                );
            }
        }
    }
    let mut written: Vec<&Value> = map.values().collect();
    if let UnmappedPolicy::Const(value) = unmapped {
        written.push(value);
    }
    if let Some(dest_enum) = &d.enum_values {
        for value in &written {
            if !dest_enum.contains(value) {
                sink.error(
                    format!("destination/{field}"),
                    format!("mapped value {value} is not a destination enum value"),
                );
            }
        }
    }
    if let Some(types) = declared_types(d) {
        for value in &written {
            if !value_has_type(value, &types) {
                sink.error(
                    format!("destination/{field}"),
                    format!(
                        "mapped value {value} does not match destination type {}",
                        types.join("|")
                    ),
                );
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let many_to_one = map.values().any(|v| !seen.insert(v.to_string()));
    if (many_to_one || matches!(unmapped, UnmappedPolicy::Const(_))) && declared_lossiness == "none"
    {
        sink.warning(
            "lossiness",
            "the map collapses several source values into one but declares lossiness none",
        );
    }
}

fn enum_key_matches(enum_value: &Value, key: &str) -> bool {
    match enum_value {
        Value::String(s) => s == key,
        Value::Number(n) => n.to_string() == key,
        Value::Bool(b) => b.to_string() == key,
        _ => false,
    }
}

fn value_has_type(value: &Value, types: &[String]) -> bool {
    types.iter().any(|t| match t.as_str() {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    })
}

/// Resolve the JSON body schema a rule targets. Problems are reported as error
/// diagnostics so that the review can list them instead of aborting.
fn body_schema<'o>(
    ops: &'o [OperationView],
    target: &Target,
    side: &str,
    sink: &mut Sink,
) -> Option<&'o ResolvedSchema> {
    let Some(op) = ops.iter().find(|candidate| {
        candidate.identity.method == target.operation.method
            && candidate.identity.path == target.operation.path
    }) else {
        sink.error(
            format!("{side}/operation"),
            format!(
                "{side} operation {} {} not found",
                target.operation.method, target.operation.path
            ),
        );
        return None;
    };
    if let Some(expected) = &target.operation.operation_id {
        if op.identity.operation_id.as_deref() != Some(expected.as_str()) {
            sink.warning(
                format!("{side}/operation"),
                format!(
                    "{side} operationId {expected:?} does not match the contract {:?}",
                    op.identity.operation_id
                ),
            );
        }
    }
    match target.location.as_str() {
        "request" => match &op.request {
            Some(body) => Some(&body.resolved),
            None => {
                sink.error(
                    format!("{side}/request"),
                    format!("{side} operation has no JSON request body"),
                );
                None
            }
        },
        "response" => {
            let status = target.response_status.as_deref().unwrap_or("200");
            match op.responses.get(status) {
                Some(body) => Some(&body.resolved),
                None => {
                    sink.error(
                        format!("{side}/response/{status}"),
                        format!("{side} operation has no JSON response {status}"),
                    );
                    None
                }
            }
        }
        other => {
            sink.error(
                format!("{side}/location"),
                format!("location must be request or response, got {other}"),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{load_openapi, Dialect};
    use crate::proposal::{load_proposal, parse_proposal_value};
    use serde_json::json;
    use std::path::PathBuf;

    fn doc(properties: Value, required: Value) -> OpenApiDoc {
        OpenApiDoc {
            path: "mem.yaml".into(),
            line_index: None,
            digest: "sha256:x".into(),
            dialect: Dialect::Openapi30,
            openapi_version: "3.0.3".into(),
            title: "t".into(),
            api_version: "1".into(),
            value: json!({
                "openapi": "3.0.3",
                "info": {"title": "t", "version": "1"},
                "paths": {"/x": {"post": {
                    "operationId": "x",
                    "requestBody": {"content": {"application/json": {"schema": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": required,
                        "properties": properties
                    }}}},
                    "responses": {"200": {"description": "ok"}}
                }}}
            }),
        }
    }

    fn proposal(op: &str, from: &str, to: &str, params: Value, lossiness: &str) -> Proposal {
        parse_proposal_value(&json!({
            "schema_version": "1",
            "kind": "proposal",
            "id": "t",
            "source_contract": {"path": "a.yaml"},
            "destination_contract": {"path": "b.yaml"},
            "rules": [{
                "id": "r1",
                "operation": op,
                "source": {"operation": {"method": "POST", "path": "/x", "operation_id": "x"}, "location": "request", "field": from},
                "destination": {"operation": {"method": "POST", "path": "/x", "operation_id": "x"}, "location": "request", "field": to},
                "parameters": params,
                "preconditions": [],
                "postconditions": [],
                "evidence": "test",
                "lossiness": lossiness,
                "reversibility": "reversible",
                "fixture_refs": []
            }]
        }))
        .unwrap()
    }

    fn errors(diags: &[Diagnostic]) -> Vec<String> {
        diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect()
    }

    #[test]
    fn every_shipped_pack_typechecks_without_errors() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for entry in std::fs::read_dir(root.join("packs")).unwrap() {
            let pack = entry.unwrap().path();
            if !pack.is_dir() {
                continue;
            }
            let proposal = load_proposal(&pack.join("proposal.json")).unwrap();
            let source = load_openapi(&pack.join("base.openapi.yaml")).unwrap();
            let dest = load_openapi(&pack.join("next.openapi.yaml")).unwrap();
            let diags = check(&proposal, &source, &dest).unwrap();
            assert!(
                errors(&diags).is_empty(),
                "{}: {:?}",
                pack.display(),
                errors(&diags)
            );
        }
    }

    #[test]
    fn rename_string_into_integer_is_rejected() {
        let source = doc(json!({"id": {"type": "string"}}), json!(["id"]));
        let dest = doc(json!({"count": {"type": "integer"}}), json!(["count"]));
        let p = proposal(
            "rename",
            "id",
            "count",
            json!({"from": "id", "to": "count"}),
            "none",
        );
        let diags = check(&p, &source, &dest).unwrap();
        assert!(
            errors(&diags)
                .iter()
                .any(|m| m.contains("string") && m.contains("integer")),
            "{diags:?}"
        );
        assert!(typecheck_proposal(&p, &source, &dest).is_err());
    }

    #[test]
    fn rename_into_required_source_field_is_rejected() {
        let source = doc(json!({"id": {"type": "string"}}), json!(["id"]));
        let dest = doc(
            json!({"id": {"type": "string"}, "user_id": {"type": "string"}}),
            json!(["id", "user_id"]),
        );
        let p = proposal(
            "rename",
            "id",
            "user_id",
            json!({"from": "id", "to": "user_id"}),
            "none",
        );
        let diags = check(&p, &source, &dest).unwrap();
        assert!(errors(&diags)
            .iter()
            .any(|m| m.contains("still requires id")));
    }

    #[test]
    fn default_value_must_satisfy_destination_schema() {
        let source = doc(json!({"name": {"type": "string"}}), json!(["name"]));
        let dest = doc(
            json!({"name": {"type": "string"}, "status": {"type": "string", "enum": ["active", "inactive"]}}),
            json!(["name", "status"]),
        );
        let bad = proposal(
            "default",
            "status",
            "status",
            json!({"field": "status", "value": "unknown"}),
            "default_inserted",
        );
        assert!(!errors(&check(&bad, &source, &dest).unwrap()).is_empty());
        let good = proposal(
            "default",
            "status",
            "status",
            json!({"field": "status", "value": "active"}),
            "default_inserted",
        );
        assert!(errors(&check(&good, &source, &dest).unwrap()).is_empty());
    }

    #[test]
    fn typecheck_requires_exhaustive_enum_map_or_policy() {
        let source = doc(
            json!({"color": {"type": "string", "enum": ["RED", "BLUE", "GREEN"]}}),
            json!(["color"]),
        );
        let dest = doc(
            json!({"color": {"type": "string", "enum": ["red", "blue", "green"]}}),
            json!(["color"]),
        );
        let partial = json!({"RED": "red", "BLUE": "blue"});
        // No policy and an uncovered source value: an error that blocks approval.
        let p = proposal(
            "enum_map",
            "color",
            "color",
            json!({"field": "color", "map": partial}),
            "none",
        );
        let errs = errors(&check(&p, &source, &dest).unwrap());
        assert!(
            errs.iter()
                .any(|m| m.contains("\"GREEN\"") && m.contains("unmapped")),
            "{errs:?}"
        );
        // An explicit policy (fail or a const value) answers the question.
        for policy in [json!("fail"), json!({"policy": "const", "value": "red"})] {
            let p = proposal(
                "enum_map",
                "color",
                "color",
                json!({"field": "color", "map": partial, "unmapped": policy}),
                "enum_many_to_one",
            );
            let errs = errors(&check(&p, &source, &dest).unwrap());
            assert!(
                errs.iter().all(|m| !m.contains("unmapped")),
                "{policy}: {errs:?}"
            );
        }
        // An exhaustive map needs no policy.
        let full = json!({"RED": "red", "BLUE": "blue", "GREEN": "green"});
        let p = proposal(
            "enum_map",
            "color",
            "color",
            json!({"field": "color", "map": full}),
            "none",
        );
        assert!(errors(&check(&p, &source, &dest).unwrap()).is_empty());
    }

    #[test]
    fn enum_map_checks_keys_values_and_exhaustiveness() {
        let source = doc(
            json!({"color": {"type": "string", "enum": ["RED", "BLUE", "GREEN"]}}),
            json!(["color"]),
        );
        let dest = doc(
            json!({"color": {"type": "string", "enum": ["red", "blue"]}}),
            json!(["color"]),
        );
        let p = proposal(
            "enum_map",
            "color",
            "color",
            json!({"field": "color", "map": {"RED": "red", "PINK": "pink"}, "unmapped": "fail"}),
            "none",
        );
        let diags = check(&p, &source, &dest).unwrap();
        let errs = errors(&diags);
        assert!(errs.iter().any(|m| m.contains("\"PINK\"")), "{errs:?}");
        assert!(errs.iter().any(|m| m.contains("\"pink\"")), "{errs:?}");
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Warning && d.message.contains("no mapping")),
            "{diags:?}"
        );
    }

    /// Map keys are text, so a key cannot tell the number 1 from the string
    /// "1": a key that matches two source enum members is a type error.
    #[test]
    fn enum_map_key_matching_two_source_members_is_a_type_error() {
        let source = doc(
            json!({"level": {"enum": [1, "1", true, "true", "x"]}}),
            json!(["level"]),
        );
        let dest = doc(
            json!({"level": {"type": "string", "enum": ["one", "yes", "x"]}}),
            json!(["level"]),
        );
        let p = proposal(
            "enum_map",
            "level",
            "level",
            json!({"field": "level", "map": {"1": "one", "true": "yes", "x": "x"}}),
            "none",
        );
        let errs = errors(&check(&p, &source, &dest).unwrap());
        for key in ["\"1\"", "\"true\""] {
            assert!(
                errs.iter()
                    .any(|m| m.contains(key) && m.contains("more than one")),
                "{key}: {errs:?}"
            );
        }
        assert!(!errs.iter().any(|m| m.contains("\"x\"")), "{errs:?}");
    }

    #[test]
    fn safe_cast_pair_must_match_declared_types() {
        let source = doc(json!({"count": {"type": "string"}}), json!(["count"]));
        let dest = doc(json!({"count": {"type": "integer"}}), json!(["count"]));
        let ok = proposal(
            "safe_cast",
            "count",
            "count",
            json!({"field": "count", "from": "string", "to": "integer"}),
            "none",
        );
        assert!(errors(&check(&ok, &source, &dest).unwrap()).is_empty());
        let wrong_from = proposal(
            "safe_cast",
            "count",
            "count",
            json!({"field": "count", "from": "integer", "to": "string"}),
            "none",
        );
        assert!(!errors(&check(&wrong_from, &source, &dest).unwrap()).is_empty());
        let unsupported = proposal(
            "safe_cast",
            "count",
            "count",
            json!({"field": "count", "from": "string", "to": "boolean"}),
            "none",
        );
        assert!(errors(&check(&unsupported, &source, &dest).unwrap())
            .iter()
            .any(|m| m.contains("not an implemented cast")));
    }

    #[test]
    fn vocabulary_is_enforced() {
        let source = doc(json!({"id": {"type": "string"}}), json!(["id"]));
        let dest = doc(json!({"user_id": {"type": "string"}}), json!(["user_id"]));
        let p = proposal(
            "rename",
            "id",
            "user_id",
            json!({"from": "id", "to": "user_id"}),
            "maybe",
        );
        let diags = check(&p, &source, &dest).unwrap();
        assert!(errors(&diags).iter().any(|m| m.contains("vocabulary")));
    }

    #[test]
    fn missing_operation_is_a_diagnostic_not_a_crash() {
        let source = doc(json!({"id": {"type": "string"}}), json!(["id"]));
        let dest = doc(json!({"user_id": {"type": "string"}}), json!(["user_id"]));
        let mut p = proposal(
            "rename",
            "id",
            "user_id",
            json!({"from": "id", "to": "user_id"}),
            "none",
        );
        p.rules[0].destination.operation.path = "/missing".into();
        let diags = check(&p, &source, &dest).unwrap();
        assert!(errors(&diags).iter().any(|m| m.contains("not found")));
    }
}
