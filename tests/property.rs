use contract_flightplan::digest::{MAX_ARRAY, MAX_RULES};
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
fn array_bound_is_enforced() {
    let items = vec![Value::String("RED".into()); MAX_ARRAY + 1];
    let err = apply_rule(
        &rule(
            "array_map",
            json!({"field":"tags","map":{"RED":"red"},"unmapped":"fail"}),
        ),
        &json!({"tags": items}),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Bound { .. }));
}

#[test]
fn unknown_mapping_cannot_execute() {
    let err = apply_rule(&rule("eval", json!({})), &json!({"a":1})).unwrap_err();
    assert!(matches!(err, Error::UnknownMappingOperation { .. }));
    let _ = MAX_RULES;
}

fn xorshift(seed: &mut u64) -> u64 {
    let mut x = *seed;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *seed = x;
    x
}

fn fuzz_value(seed: &mut u64) -> Value {
    match xorshift(seed) % 8 {
        0 => Value::Null,
        1 => Value::Bool(xorshift(seed).is_multiple_of(2)),
        2 => json!(xorshift(seed) as i32),
        3 => Value::String(format!("s{}", xorshift(seed) % 32)),
        4 => Value::Array(
            (0..(xorshift(seed) % 4))
                .map(|_| fuzz_value(seed))
                .collect(),
        ),
        _ => {
            let mut map = serde_json::Map::new();
            map.insert("id".into(), json!(format!("{}", xorshift(seed) % 16)));
            map.insert("name".into(), json!("n"));
            if xorshift(seed).is_multiple_of(2) {
                map.insert("user_id".into(), json!("dup"));
            }
            Value::Object(map)
        }
    }
}

#[test]
fn interpreter_fuzz_does_not_panic() {
    let mut seed = 0xC0FFEE_u64;
    let ops = [
        ("rename", json!({"from":"id","to":"user_id"})),
        ("drop", json!({"field":"id"})),
        (
            "safe_cast",
            json!({"field":"id","from":"string","to":"integer"}),
        ),
        ("required_field", json!({"field":"name"})),
    ];
    for _ in 0..256 {
        let input = fuzz_value(&mut seed);
        for (op, params) in &ops {
            let _ = apply_rule(&rule(op, params.clone()), &input);
        }
    }
}

#[test]
fn proposal_parser_fuzz_rejects_or_parses() {
    let mut seed = 42_u64;
    for _ in 0..64 {
        let value = fuzz_value(&mut seed);
        let _ = contract_flightplan::proposal::parse_proposal_value(&value);
    }
}

mod properties {
    use super::rule;
    use contract_flightplan::error::Error;
    use contract_flightplan::mapping::apply_rule;
    use contract_flightplan::proposal::parse_proposal_value;
    use proptest::collection::{btree_map, vec};
    use proptest::prelude::*;
    use serde_json::{json, Map, Value};

    fn key() -> impl Strategy<Value = String> {
        vec(prop::char::range('a', 'e'), 1..3).prop_map(String::from_iter)
    }

    fn json_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| json!(n)),
            vec(prop::char::range('a', 'z'), 0..5)
                .prop_map(|c| Value::String(String::from_iter(c))),
        ];
        leaf.prop_recursive(3, 24, 4, |inner| {
            prop_oneof![
                vec(inner.clone(), 0..4).prop_map(Value::Array),
                btree_map(key(), inner, 0..4)
                    .prop_map(|m| Value::Object(m.into_iter().collect::<Map<_, _>>())),
            ]
        })
    }

    fn config() -> ProptestConfig {
        ProptestConfig {
            cases: 256,
            failure_persistence: None,
            ..ProptestConfig::default()
        }
    }

    proptest! {
        #![proptest_config(config())]

        /// A rename moves exactly one value and never replaces an existing
        /// field: when the target is present the rule refuses with a collision.
        #[test]
        fn proptest_rename_never_overwrites(
            fields in btree_map(key(), json_value(), 0..6),
            from in key(),
            to in key(),
            moved in json_value(),
        ) {
            prop_assume!(from != to);
            let mut input: Map<String, Value> = fields.into_iter().collect();
            input.insert(from.clone(), moved.clone());
            let result = apply_rule(
                &rule("rename", json!({"from": from, "to": to})),
                &Value::Object(input.clone()),
            );
            if input.contains_key(&to) {
                prop_assert!(matches!(result, Err(Error::Collision { .. })), "{result:?}");
            } else {
                let output = result.unwrap().output;
                let output = output.as_object().unwrap();
                prop_assert_eq!(output.get(&to), Some(&moved));
                prop_assert!(!output.contains_key(&from));
                for (k, v) in &input {
                    if k != &from {
                        prop_assert_eq!(output.get(k), Some(v));
                    }
                }
                prop_assert_eq!(output.len(), input.len());
            }
        }

        /// The proposal parser and the strict JSON reader return a result for
        /// any input; they never panic.
        #[test]
        fn proptest_parser_never_panics(
            value in json_value(),
            text in vec(any::<char>(), 0..64).prop_map(String::from_iter),
            cut in 0usize..200,
        ) {
            let _ = parse_proposal_value(&value);
            let mut proposal = json!({
                "schema_version": "1", "kind": "proposal", "id": "p",
                "source_contract": {"path": "a"}, "destination_contract": {"path": "b"},
                "rules": [value]
            });
            let _ = parse_proposal_value(&proposal);
            proposal["rules"] = json!([]);
            let serialized = serde_json::to_string(&proposal).unwrap();
            let truncated: String = serialized.chars().take(cut).collect();
            let _ = contract_flightplan::strict_json::from_str(&truncated);
            let _ = contract_flightplan::strict_json::from_str(&text);
        }
    }
}
