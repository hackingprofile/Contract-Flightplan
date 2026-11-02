//! Reordering a contract (object keys, operations, properties) changes no
//! identity and produces no change.

use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The same JSON value with every object's keys in reverse order.
fn reversed(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut pairs: Vec<(&String, &Value)> = map.iter().collect();
            pairs.reverse();
            Value::Object(
                pairs
                    .into_iter()
                    .map(|(k, v)| (k.clone(), reversed(v)))
                    .collect::<Map<String, Value>>(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(reversed).collect()),
        other => other.clone(),
    }
}

/// Write `value` with its keys in reverse order. serde_json's map is sorted,
/// so the text is built by hand to keep the reversed order on disk.
fn write_reversed_json(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().rev().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push(':');
                write_reversed_json(v, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_reversed_json(v, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).unwrap()),
    }
}

#[test]
fn reorder_only_contract_has_no_property_changes_and_same_identity() {
    let dir = std::env::temp_dir().join(format!("cfp-reorder-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut compared = 0;
    for pack in [
        "rename-user-id",
        "array-tags",
        "split-full-name",
        "page-envelope",
    ] {
        let base = root().join("packs").join(pack).join("base.openapi.yaml");
        let text = fs::read_to_string(&base).unwrap();
        let value: Value = yaml_serde::from_str(&text).unwrap();
        assert_eq!(reversed(&reversed(&value)), value);
        let mut reordered = String::new();
        write_reversed_json(&value, &mut reordered);
        assert_ne!(
            reordered,
            serde_json::to_string(&value).unwrap(),
            "{pack}: the text really is reordered"
        );
        let copy = dir.join(format!("{pack}.reordered.json"));
        fs::write(&copy, &reordered).unwrap();

        let diff =
            serde_json::to_value(contract_flightplan::diff::compare_paths(&base, &copy).unwrap())
                .unwrap();
        assert_eq!(diff["added_operations"], Value::Array(vec![]), "{pack}");
        assert_eq!(diff["removed_operations"], Value::Array(vec![]), "{pack}");
        let operations = diff["operations"].as_array().unwrap();
        assert!(!operations.is_empty(), "{pack}");
        for op in operations {
            assert_eq!(op["identity"], op["destination_identity"], "{pack}");
            let mut bodies: Vec<&Value> = op["responses"].as_object().unwrap().values().collect();
            if !op["request"].is_null() {
                bodies.push(&op["request"]);
            }
            for body in bodies {
                assert_eq!(body["compatibility"], "unchanged", "{pack}: {body:#}");
                assert_eq!(body["changes"], Value::Array(vec![]), "{pack}: {body:#}");
                compared += 1;
            }
        }
    }
    assert!(compared >= 8, "{compared} bodies compared");
}
