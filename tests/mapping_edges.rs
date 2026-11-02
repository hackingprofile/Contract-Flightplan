//! Interpreter edges that pack fixtures cannot reach because source-schema
//! validation refuses the input first: each operation still refuses on its
//! own.

use contract_flightplan::digest::MAX_ARRAY;
use contract_flightplan::error::Error;
use contract_flightplan::mapping::apply_rule;
use contract_flightplan::parse::OpIdentity;
use contract_flightplan::proposal::{Rule, Target};
use serde_json::{json, Value};

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
fn safe_cast_and_unit_conversion_refuse_explicit_null() {
    let cast = apply_rule(
        &rule(
            "safe_cast",
            json!({"field": "count", "from": "string", "to": "integer"}),
        ),
        &json!({"count": null}),
    );
    assert!(matches!(cast, Err(Error::ExplicitNull { .. })), "{cast:?}");
    let unit = apply_rule(
        &rule(
            "unit_conversion",
            json!({"field": "mass", "from_unit": "kg", "to_unit": "g", "factor_num": 1000, "factor_den": 1, "rounding": "exact"}),
        ),
        &json!({"mass": null}),
    );
    assert!(matches!(unit, Err(Error::ExplicitNull { .. })), "{unit:?}");
}

#[test]
fn safe_cast_number_to_integer_refuses_fractions() {
    let cast = rule(
        "safe_cast",
        json!({"field": "n", "from": "number", "to": "integer"}),
    );
    let fraction = apply_rule(&cast, &json!({"n": 1.5}));
    assert!(
        matches!(fraction, Err(Error::Malformed { .. })),
        "{fraction:?}"
    );
    let whole = apply_rule(&cast, &json!({"n": 2.0})).unwrap();
    assert_eq!(whole.output, json!({"n": 2}));
}

#[test]
fn merge_joins_with_its_separator() {
    let merged = apply_rule(
        &rule(
            "merge",
            json!({"fields": ["first", "last"], "into": "full", "separator": " "}),
        ),
        &json!({"first": "Ada", "last": "Lovelace"}),
    )
    .unwrap();
    assert_eq!(merged.output, json!({"full": "Ada Lovelace"}));
    assert_eq!(merged.lossiness, vec!["merged".to_string()]);
}

#[test]
fn array_map_const_fallback_records_many_to_one() {
    let mapped = apply_rule(
        &rule(
            "array_map",
            json!({"field": "tags", "map": {"RED": "red"}, "unmapped": {"policy": "const", "value": "other"}}),
        ),
        &json!({"tags": ["RED", "PINK"]}),
    )
    .unwrap();
    assert_eq!(mapped.output, json!({"tags": ["red", "other"]}));
    assert_eq!(mapped.lossiness, vec!["enum_many_to_one".to_string()]);
    // A map that covers every item is not lossy.
    let exact = apply_rule(
        &rule(
            "array_map",
            json!({"field": "tags", "map": {"RED": "red"}, "unmapped": {"policy": "const", "value": "other"}}),
        ),
        &json!({"tags": ["RED"]}),
    )
    .unwrap();
    assert!(exact.lossiness.is_empty());
}

#[test]
fn array_filter_bound_is_enforced() {
    let filter = rule("array_filter", json!({"field": "tags", "drop_null": true}));
    let over = apply_rule(
        &filter,
        &json!({ "tags": vec![Value::Null; MAX_ARRAY + 1] }),
    );
    assert!(matches!(over, Err(Error::Bound { .. })), "{over:?}");
    let at = apply_rule(&filter, &json!({ "tags": vec![json!("a"); MAX_ARRAY] })).unwrap();
    assert_eq!(at.output["tags"].as_array().unwrap().len(), MAX_ARRAY);
}

// number -> integer must not pass integer-backed JSON numbers through f64, where 2^53 + 1
// becomes 2^53 and 2^63 saturates to i64::MAX.
#[test]
fn safe_cast_number_to_integer_is_exact_and_refuses_out_of_range() {
    let cast = rule(
        "safe_cast",
        json!({"field": "n", "from": "number", "to": "integer"}),
    );
    let ok = |v: Value| apply_rule(&cast, &json!({ "n": v })).map(|r| r.output);
    // exact across 2^53 and at both i64 extremes
    for n in [
        9_007_199_254_740_992i64,
        9_007_199_254_740_993,
        -9_007_199_254_740_993,
        i64::MAX,
        i64::MIN,
    ] {
        assert_eq!(ok(json!(n)).unwrap(), json!({ "n": n }), "{n}");
    }
    // unsigned values above i64::MAX are refused, never saturated
    for u in [9_223_372_036_854_775_808u64, u64::MAX] {
        let r = ok(json!(u));
        assert!(
            matches!(&r, Err(Error::Malformed { kind, .. }) if kind == "overflow"),
            "{u}: {r:?}"
        );
    }
    // floating-backed: integral values convert, fractions and 2^63 (as a float) do not
    assert_eq!(ok(json!(-2.0)).unwrap(), json!({"n": -2}));
    assert_eq!(
        ok(json!(-9_223_372_036_854_775_808.0f64)).unwrap(),
        json!({ "n": i64::MIN })
    );
    for f in [0.5f64, -1.25, 9_223_372_036_854_775_808.0, 1e300] {
        let r = ok(json!(f));
        assert!(matches!(r, Err(Error::Malformed { .. })), "{f}: {r:?}");
    }
}

// A negative denominator must not invert floor and ceil (1 / -2 floors to -1), and
// i64::MIN * 1 / -1 must return a typed error instead of panicking at the remainder.
#[test]
fn unit_conversion_handles_signed_denominators_without_panic() {
    let conv = |n: i64, num: i64, den: i64, rounding: &str| {
        apply_rule(
            &rule(
                "unit_conversion",
                json!({"field": "n", "from_unit": "a", "to_unit": "b", "factor_num": num, "factor_den": den, "rounding": rounding}),
            ),
            &json!({ "n": n }),
        )
        .map(|r| r.output["n"].as_i64().unwrap())
    };
    // floor/ceil/round agree with the mathematical value n * num / den for every sign pairing
    for (n, num, den) in [
        (1, 1, -2),
        (-1, 1, -2),
        (1, -1, 2),
        (-1, -1, -2),
        (7, 1, -3),
        (-7, 1, 3),
        (5, 3, -4),
    ] {
        let exact = (n * num) as f64 / den as f64;
        assert_eq!(
            conv(n, num, den, "floor").unwrap() as f64,
            exact.floor(),
            "floor {n}*{num}/{den}"
        );
        assert_eq!(
            conv(n, num, den, "ceil").unwrap() as f64,
            exact.ceil(),
            "ceil {n}*{num}/{den}"
        );
        // round: half up (toward +inf), the existing positive-denominator tie policy
        assert_eq!(
            conv(n, num, den, "round").unwrap() as f64,
            (exact + 0.5).floor(),
            "round {n}*{num}/{den}"
        );
    }
    assert_eq!(conv(6, 1, -3, "exact").unwrap(), -2);
    assert!(matches!(
        conv(7, 1, -3, "exact"),
        Err(Error::Malformed { .. })
    ));
    // the positive-denominator behaviour is unchanged, ties included
    assert_eq!(conv(5, 1, 2, "round").unwrap(), 3);
    assert_eq!(conv(-5, 1, 2, "round").unwrap(), -2);
    assert_eq!(conv(i64::MAX, 1, 1, "exact").unwrap(), i64::MAX);
    // i64::MIN / -1 overflows i64: a typed error for every rounding, never a panic
    for rounding in ["exact", "floor", "ceil", "round"] {
        let r = std::panic::catch_unwind(|| conv(i64::MIN, 1, -1, rounding));
        let r = r.unwrap_or_else(|_| panic!("{rounding} panicked"));
        assert!(
            matches!(&r, Err(Error::Malformed { kind, .. }) if kind == "overflow"),
            "{rounding}: {r:?}"
        );
    }
    // the existing multiply-overflow refusal and the zero-denominator refusal stay
    assert!(matches!(
        conv(i64::MAX, 2, 1, "exact"),
        Err(Error::Malformed { .. })
    ));
    assert!(matches!(conv(1, 1, 0, "exact"), Err(Error::Invalid { .. })));
}

/// string -> number keeps the exact value or refuses: integers stay exact JSON
/// integers at any i64/u64 size, decimals keep their value, and a string whose
/// value no JSON number can hold exactly is refused, never rounded.
#[test]
fn safe_cast_string_to_number_keeps_the_exact_value_or_refuses() {
    let cast = rule(
        "safe_cast",
        json!({"field": "n", "from": "string", "to": "number"}),
    );
    let ok = |s: &str| apply_rule(&cast, &json!({ "n": s })).map(|r| r.output["n"].clone());
    assert_eq!(
        ok("9007199254740993").unwrap(),
        json!(9_007_199_254_740_993i64)
    );
    assert_eq!(ok("-9223372036854775808").unwrap(), json!(i64::MIN));
    assert_eq!(ok("18446744073709551615").unwrap(), json!(u64::MAX));
    assert_eq!(ok("42").unwrap(), json!(42));
    assert_eq!(ok("1.5").unwrap(), json!(1.5));
    assert_eq!(ok("-0.25").unwrap(), json!(-0.25));
    assert_eq!(ok("0.1").unwrap(), json!(0.1));
    assert_eq!(ok("2.50").unwrap(), json!(2.5));
    assert_eq!(ok("1e3").unwrap(), json!(1000.0));
    for lossy in [
        "9007199254740993.0",
        "18446744073709551616",
        "0.1000000000000000000000001",
        "1e400",
        "NaN",
        "inf",
    ] {
        let r = ok(lossy);
        assert!(
            matches!(&r, Err(Error::Malformed { kind, .. }) if kind == "overflow"),
            "{lossy}: {r:?}"
        );
    }
}

/// array_map works "as enum_map, applied to each item" (docs/mapping.md), so a
/// null item is refused as `explicit_null`, like a null enum_map field.
#[test]
fn array_map_null_item_is_explicit_null_like_enum_map() {
    let map = rule(
        "array_map",
        json!({"field": "tags", "map": {"a": "A"}, "unmapped": "fail"}),
    );
    let r = apply_rule(&map, &json!({"tags": ["a", null]}));
    assert!(
        matches!(&r, Err(Error::ExplicitNull { location, .. }) if location.ends_with("/1")),
        "{r:?}"
    );
    let enum_map = rule(
        "enum_map",
        json!({"field": "tags", "map": {"a": "A"}, "unmapped": "fail"}),
    );
    let r = apply_rule(&enum_map, &json!({ "tags": null }));
    assert!(matches!(&r, Err(Error::ExplicitNull { .. })), "{r:?}");
}

/// array_filter refuses a null field as `explicit_null`, like every other
/// operation; only a non-array, non-null value is `malformed`.
#[test]
fn array_filter_null_field_is_explicit_null() {
    let filter = rule("array_filter", json!({"field": "tags", "drop_null": true}));
    let r = apply_rule(&filter, &json!({ "tags": null }));
    assert!(matches!(&r, Err(Error::ExplicitNull { .. })), "{r:?}");
    let r = apply_rule(&filter, &json!({ "tags": "a" }));
    assert!(matches!(&r, Err(Error::Malformed { .. })), "{r:?}");
    let ok = apply_rule(&filter, &json!({ "tags": ["a", null, "b"] })).unwrap();
    assert_eq!(ok.output, json!({ "tags": ["a", "b"] }));
}
