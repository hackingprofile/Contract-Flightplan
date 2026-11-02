//! JSON Schema constraints the contracts declare are enforced, not ignored:
//! numeric, string and array bounds, `pattern`, and the formats this tool
//! names. A constraint it cannot evaluate is reported as unsupported.

use std::fs;
use std::path::PathBuf;

use contract_flightplan::parse::{load_openapi, operations};
use contract_flightplan::schema::{validate_instance, ResolvedSchema};
use serde_json::{json, Value};

fn body_schema(openapi: &str, properties: Value) -> ResolvedSchema {
    let dir = std::env::temp_dir().join(format!(
        "cfp-validation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
            + "-"
            + &{
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    .to_string()
            }
    ));
    fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("contract.json");
    let doc = json!({
        "openapi": openapi,
        "info": {"title": "Bounds", "version": "1"},
        "paths": {"/things": {"post": {
            "operationId": "createThing",
            "requestBody": {"content": {"application/json": {"schema": {
                "type": "object",
                "properties": properties
            }}}},
            "responses": {"200": {"description": "ok"}}
        }}}
    });
    fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    let contract = load_openapi(&path).unwrap();
    let ops = operations(&contract).unwrap();
    ops[0].request.as_ref().unwrap().resolved.clone()
}

fn outcome(schema: &ResolvedSchema, body: Value) -> String {
    match validate_instance(schema, &body, "x") {
        Ok(()) => "success".into(),
        Err(err) => err.outcome_name().into(),
    }
}

#[test]
fn schema_validation_enforces_numeric_and_string_bounds() {
    let schema = body_schema(
        "3.1.0",
        json!({
            "count": {"type": "integer", "minimum": 1, "maximum": 10},
            "score": {"type": "number", "exclusiveMinimum": 0, "exclusiveMaximum": 1},
            "name": {"type": "string", "minLength": 2, "maxLength": 5},
            "code": {"type": "string", "pattern": "^[A-Z]{3}$"},
            "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 2, "uniqueItems": true}
        }),
    );
    assert!(schema.unsupported.is_empty(), "{:?}", schema.unsupported);
    let valid = json!({"count": 1, "score": 0.5, "name": "Ada", "code": "ABC", "tags": ["a"]});
    assert_eq!(outcome(&schema, valid.clone()), "success");
    let with = |key: &str, value: Value| {
        let mut body = valid.clone();
        body[key] = value;
        body
    };
    for (key, value) in [
        ("count", json!(0)),
        ("count", json!(11)),
        ("score", json!(0)),
        ("score", json!(1)),
        ("name", json!("A")),
        ("name", json!("Adalovelace")),
        ("code", json!("abc")),
        ("code", json!("ABCD")),
        ("tags", json!([])),
        ("tags", json!(["a", "b", "c"])),
        ("tags", json!(["a", "a"])),
    ] {
        assert_eq!(
            outcome(&schema, with(key, value.clone())),
            "schema_validation",
            "{key} = {value}"
        );
    }
    // Bounds are inclusive unless exclusive; string length counts characters.
    assert_eq!(outcome(&schema, with("count", json!(10))), "success");
    assert_eq!(outcome(&schema, with("name", json!("Ådäłö"))), "success");

    // OpenAPI 3.0 spells exclusivity as a boolean beside minimum/maximum.
    let schema = body_schema(
        "3.0.3",
        json!({"ratio": {"type": "number", "minimum": 0, "exclusiveMinimum": true, "maximum": 1}}),
    );
    assert!(schema.unsupported.is_empty(), "{:?}", schema.unsupported);
    assert_eq!(outcome(&schema, json!({"ratio": 0})), "schema_validation");
    assert_eq!(outcome(&schema, json!({"ratio": 1})), "success");
}

#[test]
fn format_date_time_and_uuid_are_validated() {
    let schema = body_schema(
        "3.0.3",
        json!({
            "day": {"type": "string", "format": "date"},
            "at": {"type": "string", "format": "date-time"},
            "id": {"type": "string", "format": "uuid"},
            "mail": {"type": "string", "format": "email"},
            "other": {"type": "string", "format": "x-house-style"}
        }),
    );
    let valid = json!({
        "day": "2028-02-29",
        "at": "2026-10-01T12:30:00.5+05:30",
        "id": "123e4567-e89b-12d3-a456-426614174000",
        "mail": "ada@example.org",
        "other": "anything"
    });
    assert_eq!(outcome(&schema, valid.clone()), "success");
    for (key, value) in [
        ("day", "2026-02-29"),
        ("day", "2026-13-01"),
        ("day", "26-01-01"),
        ("at", "2026-10-01 12:30"),
        ("at", "2026-10-01T25:00:00Z"),
        ("at", "2026-10-01T12:30:00"),
        ("id", "123e4567e89b12d3a456426614174000"),
        ("id", "123e4567-e89b-12d3-a456-42661417400g"),
        ("mail", "ada.example.org"),
        ("mail", "ada@"),
    ] {
        let mut body = valid.clone();
        body[key] = json!(value);
        assert_eq!(
            outcome(&schema, body),
            "schema_validation",
            "{key} = {value}"
        );
    }

    // A pattern this tool cannot evaluate is visible as unsupported, never
    // silently treated as satisfied.
    let schema = body_schema(
        "3.0.3",
        json!({"code": {"type": "string", "pattern": "^(?=A)[A-Z]+$"}}),
    );
    let notes: Vec<String> = schema
        .all_unsupported()
        .into_iter()
        .map(|n| n.construct)
        .collect();
    assert_eq!(notes, vec!["pattern".to_string()]);
}

/// A schema without `type` constrains nothing about the type (JSON Schema and
/// OpenAPI 3.0/3.1 agree): an enum-only property accepts its values, and the
/// other keywords still apply.
#[test]
fn an_untyped_schema_is_unconstrained_by_type_but_keeps_its_keywords() {
    for dialect in ["3.0.3", "3.1.0"] {
        let schema = body_schema(
            dialect,
            json!({
                "status": {"enum": ["active", "closed"]},
                "note": {"description": "anything"},
                "code": {"minLength": 2}
            }),
        );
        assert!(schema.unsupported.is_empty(), "{:?}", schema.unsupported);
        let ok = json!({"status": "active", "note": 3, "code": "AB"});
        assert_eq!(outcome(&schema, ok), "success", "{dialect}");
        for note in [
            json!("text"),
            json!({"k": 1}),
            json!([1]),
            json!(true),
            json!(null),
        ] {
            assert_eq!(
                outcome(&schema, json!({"status": "closed", "note": note})),
                "success",
                "{dialect}: untyped note {note}"
            );
        }
        assert_eq!(
            outcome(&schema, json!({"status": "lots"})),
            "unknown_enum",
            "{dialect}"
        );
        assert_eq!(
            outcome(&schema, json!({"code": "A"})),
            "schema_validation",
            "{dialect}: minLength still applies to a string"
        );
    }
}

/// `minimum`/`maximum` are compared exactly: integers beyond 2^53 and near
/// the 64-bit limits, and integers against fractional bounds.
#[test]
fn numeric_bounds_are_compared_exactly() {
    let schema = body_schema(
        "3.1.0",
        json!({
            "max53": {"type": "integer", "maximum": 9_007_199_254_740_992_i64},
            "max64": {"type": "integer", "maximum": 9_223_372_036_854_775_806_i64},
            "min64": {"type": "integer", "minimum": -9_223_372_036_854_775_807_i64},
            "maxu": {"type": "integer", "maximum": 18_446_744_073_709_551_614_u64},
            "xmax": {"type": "integer", "exclusiveMaximum": 9_007_199_254_740_993_i64},
            "frac": {"type": "number", "minimum": 0.5, "maximum": 1.5}
        }),
    );
    let cases = [
        (
            json!({"max53": 9_007_199_254_740_993_i64}),
            "schema_validation",
        ),
        (json!({"max53": 9_007_199_254_740_992_i64}), "success"),
        (
            json!({"max64": 9_223_372_036_854_775_807_i64}),
            "schema_validation",
        ),
        (json!({"min64": i64::MIN}), "schema_validation"),
        (json!({"maxu": u64::MAX}), "schema_validation"),
        (
            json!({"xmax": 9_007_199_254_740_993_i64}),
            "schema_validation",
        ),
        (json!({"xmax": 9_007_199_254_740_992_i64}), "success"),
        (json!({"frac": 0}), "schema_validation"),
        (json!({"frac": 1}), "success"),
        (json!({"frac": 2}), "schema_validation"),
        (json!({"frac": 1.5}), "success"),
    ];
    for (body, expected) in cases {
        assert_eq!(outcome(&schema, body.clone()), expected, "{body}");
    }
}

/// The validator follows JSON Schema and ECMA-262 where it differed: ASCII
/// `\d` and `\w`, numbers compared by value, `1.0` is an integer, no control
/// or format characters in an email, and a leap second only at 23:59:60 UTC.
#[test]
fn validator_matches_json_schema_where_it_differed() {
    let schema = body_schema(
        "3.1.0",
        json!({
            "digits": {"type": "string", "pattern": r"^\d+$"},
            "word": {"type": "string", "pattern": r"^\w+$"},
            "bounded": {"type": "string", "pattern": r"^ab\b"},
            "class": {"type": "string", "pattern": r"^[\w-]+$"},
            "set": {"type": "array", "uniqueItems": true, "items": {"type": "number"}},
            "count": {"type": "integer"},
            "one": {"enum": [1]},
            "email": {"type": "string", "format": "email"},
            "at": {"type": "string", "format": "date-time"}
        }),
    );
    let cases = [
        (json!({"digits": "123"}), "success"),
        (
            json!({"digits": "\u{661}\u{662}\u{663}"}),
            "schema_validation",
        ),
        (json!({"word": "pass_1"}), "success"),
        (
            json!({"word": "\u{43f}\u{430}\u{440}"}),
            "schema_validation",
        ),
        (json!({"bounded": "ab\u{e9}"}), "success"),
        (json!({"class": "a-b_c"}), "success"),
        (json!({"class": "a\u{e9}"}), "schema_validation"),
        (json!({"set": [1, 1.0]}), "schema_validation"),
        (json!({"set": [1, 2.5]}), "success"),
        (json!({"count": 1.0}), "success"),
        (json!({"count": 1.5}), "malformed"),
        (json!({"one": 1.0}), "success"),
        (json!({"one": 2}), "unknown_enum"),
        (json!({"email": "a@example.com"}), "success"),
        (
            json!({"email": "a\u{202e}@example.com"}),
            "schema_validation",
        ),
        (json!({"email": "a\u{0}@example.com"}), "schema_validation"),
        (json!({"at": "2026-10-30T23:59:60Z"}), "success"),
        (json!({"at": "2026-10-30T18:29:60-05:30"}), "success"),
        (json!({"at": "2026-10-30T12:00:60Z"}), "schema_validation"),
    ];
    let mut wrong = Vec::new();
    for (body, expected) in cases {
        let actual = outcome(&schema, body.clone());
        if actual != expected {
            wrong.push(format!("{body}: expected {expected}, got {actual}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
