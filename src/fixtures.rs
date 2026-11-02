use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::digest::MAX_ARRAY;
use crate::error::{Error, Result};
use crate::parse::{load_openapi, operations};
use crate::schema::ResolvedSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedFixtures {
    pub schema_version: String,
    pub kind: String,
    /// The u64 seed the sample values were drawn with; the same seed and
    /// schema always give the same items.
    pub seed: u64,
    pub items: Vec<GeneratedItem>,
    pub coverage: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedItem {
    pub id: String,
    pub expect: String,
    pub body: Value,
}

/// The resolved schema of one body of a contract: the request body, or the
/// response body for `status` (default 200).
pub fn body_schema_for(
    source: &Path,
    method: &str,
    path: &str,
    location: &str,
    status: Option<&str>,
) -> Result<ResolvedSchema> {
    let doc = load_openapi(source)?;
    let ops = operations(&doc)?;
    let op = ops
        .iter()
        .find(|candidate| candidate.identity.method == method && candidate.identity.path == path)
        .ok_or_else(|| Error::Unknown {
            what: "operation".into(),
            location: format!("{method} {path}"),
            message: "operation not found in the contract".into(),
        })?;
    let schema = match location {
        "request" => op
            .request
            .as_ref()
            .map(|body| &body.resolved)
            .ok_or_else(|| Error::Invalid {
                message: "no request body".into(),
            })?,
        "response" => {
            let code = status.unwrap_or("200");
            op.responses
                .get(code)
                .map(|body| &body.resolved)
                .ok_or_else(|| Error::Invalid {
                    message: format!("no response {code}"),
                })?
        }
        other => {
            return Err(Error::Unsupported {
                construct: "location".into(),
                location: other.into(),
                message: "location must be request or response".into(),
            });
        }
    };
    Ok(schema.clone())
}

pub fn generate_for_operation(
    source: &Path,
    method: &str,
    path: &str,
    location: &str,
    status: Option<&str>,
    seed: u64,
) -> Result<GeneratedFixtures> {
    let schema = body_schema_for(source, method, path, location, status)?;
    Ok(generate_from_schema(&schema, seed))
}

/// SplitMix64: a small, well-known deterministic generator. The stream
/// depends only on the seed, so generated fixtures reproduce exactly.
pub struct SeededRng(u64);

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

pub fn generate_from_schema(schema: &ResolvedSchema, seed: u64) -> GeneratedFixtures {
    let mut rng = SeededRng::new(seed);
    let mut items = Vec::new();
    let positive = sample_object(schema, &mut rng);
    items.push(GeneratedItem {
        id: "generated-positive".into(),
        expect: "success".into(),
        body: Value::Object(positive.clone()),
    });
    if let Some(field) = schema.required.first() {
        let mut missing = positive.clone();
        missing.remove(field);
        items.push(GeneratedItem {
            id: format!("generated-missing-{field}"),
            expect: "missing".into(),
            body: Value::Object(missing),
        });
        let mut nulls = positive.clone();
        nulls.insert(field.clone(), Value::Null);
        // The expectation is what the contract itself says about null: a
        // nullable field accepts it; any other field refuses it.
        let nullable = schema.properties.get(field).is_some_and(|c| {
            c.types.is_empty() || c.nullable || c.types.iter().any(|t| t == "null")
        });
        items.push(GeneratedItem {
            id: format!("generated-null-{field}"),
            expect: if nullable {
                "success"
            } else {
                "schema_validation"
            }
            .into(),
            body: Value::Object(nulls),
        });
        if let Some(child) = schema.properties.get(field) {
            // An untyped field accepts every type, so there is no malformed value for it.
            if !child.types.is_empty() {
                let mut malformed = positive.clone();
                malformed.insert(field.clone(), wrong_type(child));
                items.push(GeneratedItem {
                    id: format!("generated-malformed-{field}"),
                    expect: "malformed".into(),
                    body: Value::Object(malformed),
                });
            }
            if child.enum_values.is_some() {
                let mut body = positive.clone();
                body.insert(field.clone(), Value::String("__unmapped__".into()));
                items.push(GeneratedItem {
                    id: format!("generated-enum-negative-{field}"),
                    expect: "unknown_enum".into(),
                    body: Value::Object(body),
                });
            }
        }
    }
    boundary_items(schema, &positive, &mut rng, &mut items);
    // The expectation is what the contract itself reports for each body, so
    // a constraint the sample cannot meet shows up instead of being assumed.
    for item in &mut items {
        item.expect = match crate::schema::validate_instance(schema, &item.body, &item.id) {
            Ok(()) => "success".into(),
            Err(err) => err.outcome_name().into(),
        };
    }
    let coverage = items.iter().map(|item| item.expect.clone()).collect();
    GeneratedFixtures {
        schema_version: "1".into(),
        kind: "generated_fixtures".into(),
        seed,
        items,
        coverage,
    }
}

/// Edge values for each top-level property, every other field kept from the
/// positive fixture: the empty string, zero and the largest integer, the empty
/// array, an array at the item bound (expected to pass) and one item over it
/// (expected `bound`). Enum-constrained properties are skipped: their edges
/// are the enum values the positive already draws from.
fn boundary_items(
    schema: &ResolvedSchema,
    positive: &Map<String, Value>,
    rng: &mut SeededRng,
    items: &mut Vec<GeneratedItem>,
) {
    let has = |child: &ResolvedSchema, t: &str| child.types.iter().any(|x| x == t);
    for (field, child) in &schema.properties {
        if child.enum_values.is_some() || child.is_unsupported() {
            continue;
        }
        let mut cases: Vec<(&str, Value, &str)> = Vec::new();
        if has(child, "string") {
            cases.push(("empty", Value::String(String::new()), "success"));
        }
        if has(child, "integer") {
            cases.push(("zero", Value::Number(0.into()), "success"));
            cases.push(("max", Value::Number(i64::MAX.into()), "success"));
        } else if has(child, "number") {
            cases.push(("zero", Value::Number(0.into()), "success"));
        }
        if has(child, "array") {
            if let Some(item_schema) = &child.items {
                let one = sample_value(item_schema, rng);
                cases.push(("empty", Value::Array(Vec::new()), "success"));
                cases.push((
                    "at-limit",
                    Value::Array(vec![one.clone(); MAX_ARRAY]),
                    "success",
                ));
                cases.push((
                    "over-limit",
                    Value::Array(vec![one; MAX_ARRAY + 1]),
                    "bound",
                ));
            }
        }
        for (case, value, expect) in cases {
            let mut body = positive.clone();
            body.insert(field.clone(), value);
            items.push(GeneratedItem {
                id: format!("generated-boundary-{field}-{case}"),
                expect: expect.into(),
                body: Value::Object(body),
            });
        }
    }
}

fn sample_object(schema: &ResolvedSchema, rng: &mut SeededRng) -> Map<String, Value> {
    let mut map = Map::new();
    for (name, child) in &schema.properties {
        map.insert(name.clone(), sample_value(child, rng));
    }
    map
}

/// A value of the schema's type drawn from the seeded stream.
fn sample_value(schema: &ResolvedSchema, rng: &mut SeededRng) -> Value {
    let draw = rng.next_u64();
    if let Some(values) = &schema.enum_values {
        if !values.is_empty() {
            return values[(draw % values.len() as u64) as usize].clone();
        }
    }
    if schema.types.iter().any(|t| t == "array") {
        if let Some(items) = &schema.items {
            return Value::Array(vec![sample_value(items, rng)]);
        }
        return Value::Array(vec![]);
    }
    if schema.types.iter().any(|t| t == "object")
        || (schema.types.is_empty() && !schema.properties.is_empty())
    {
        return Value::Object(sample_object(schema, rng));
    }
    if schema.types.iter().any(|t| t == "integer" || t == "number") {
        return Value::Number((draw % 1000).into());
    }
    if schema.types.iter().any(|t| t == "boolean") {
        return Value::Bool(draw.is_multiple_of(2));
    }
    Value::String(format!("s-{:08x}", draw as u32))
}

fn wrong_type(schema: &ResolvedSchema) -> Value {
    if schema.types.iter().any(|t| t == "string") {
        Value::Number(1.into())
    } else {
        Value::String("not-the-declared-type".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{Dialect, OpenApiDoc};
    use serde_json::json;

    #[test]
    fn enum_negative_keeps_other_required_fields() {
        let doc = OpenApiDoc {
            path: "mem.yaml".into(),
            line_index: None,
            digest: "sha256:x".into(),
            dialect: Dialect::Openapi30,
            openapi_version: "3.0.3".into(),
            title: "t".into(),
            api_version: "1".into(),
            value: json!({"openapi":"3.0.3","info":{"title":"t","version":"1"},"paths":{}}),
        };
        let schema = crate::schema::resolve_schema(
            &doc,
            &json!({
                "type": "object",
                "required": ["color", "label"],
                "properties": {
                    "color": {"type": "string", "enum": ["RED", "BLUE"]},
                    "label": {"type": "string"}
                }
            }),
            "/s",
        )
        .unwrap();
        let generated = generate_from_schema(&schema, 0);
        let item = generated
            .items
            .iter()
            .find(|i| i.id == "generated-enum-negative-color")
            .expect("enum-negative fixture");
        assert_eq!(item.expect, "unknown_enum");
        assert_eq!(item.body["color"], "__unmapped__");
        // The other required field keeps the positive fixture's (seeded) value.
        let positive = &generated.items[0];
        assert_eq!(positive.expect, "success");
        assert!(item.body["label"].is_string());
        assert_eq!(item.body["label"], positive.body["label"]);
    }
}
