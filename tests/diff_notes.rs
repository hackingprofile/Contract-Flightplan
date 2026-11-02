//! Unsupported-construct notes are emitted once each, with their own
//! construct name and location, and an unsupported body derives no property
//! additions or removals from the side the tool cannot read.

use std::fs;
use std::path::{Path, PathBuf};

use contract_flightplan::diff::compare_paths;
use serde_json::{json, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-notes-{name}-{}-{}",
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
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every object in the value that looks like a note: `construct` + `message`.
fn notes(value: &Value, out: &mut Vec<Value>) {
    match value {
        Value::Object(map) => {
            if map.contains_key("construct") && map.contains_key("message") {
                out.push(value.clone());
            }
            map.values().for_each(|v| notes(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| notes(v, out)),
        _ => {}
    }
}

#[test]
fn recursive_reference_note_is_emitted_once_with_its_location() {
    let recursive = root().join("fixtures/negatives/recursive.openapi.yaml");
    let diff = serde_json::to_value(compare_paths(&recursive, &recursive).unwrap()).unwrap();
    let mut found = Vec::new();
    notes(&diff, &mut found);
    let keys: Vec<(String, String)> = found
        .iter()
        .map(|n| {
            (
                n["construct"].as_str().unwrap().to_string(),
                n["location"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let mut distinct = keys.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(keys.len(), distinct.len(), "a note is repeated: {found:#?}");
    let base = "/paths/~1nodes/post";
    assert_eq!(
        distinct,
        vec![
            (
                "recursive_reference".to_string(),
                format!("{base}/requestBody/content/application~1json/schema/properties/child")
            ),
            (
                "recursive_reference".to_string(),
                format!("{base}/responses/200/content/application~1json/schema/properties/child")
            ),
        ],
        "{found:#?}"
    );
}

#[test]
fn report_counts_each_unsupported_construct_once() {
    let recursive = root().join("fixtures/negatives/recursive.openapi.yaml");
    let report =
        contract_flightplan::report::build_report(&recursive, &recursive, None, None, None, None)
            .unwrap();
    let html = contract_flightplan::report::render_html(&report);
    assert!(html.contains("Unsupported constructs: 2."), "{html}");
}

fn contract(dir: &Path, name: &str, schema: Value, components: Value) -> PathBuf {
    let doc = json!({
        "openapi": "3.0.3",
        "info": {"title": "allOf", "version": "1"},
        "paths": {"/users": {"post": {
            "operationId": "createUser",
            "requestBody": {"content": {"application/json": {"schema": schema}}},
            "responses": {"200": {"description": "ok"}}
        }}},
        "components": {"schemas": components}
    });
    let path = dir.join(name);
    fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    path
}

#[test]
fn allof_body_reports_unknown_and_derives_no_removals() {
    let flat = json!({"type": "object", "properties": {
        "a": {"type": "string"}, "b": {"type": "string"}
    }, "required": ["a"]});
    let wrapped = json!({"allOf": [{"$ref": "#/components/schemas/Base"}]});
    let components = json!({"Base": flat.clone()});
    for (name, before, after) in [
        ("into", flat.clone(), wrapped.clone()),
        ("out-of", wrapped, flat),
    ] {
        let dir = scratch(name);
        let base = contract(&dir, "base.json", before, components.clone());
        let next = contract(&dir, "next.json", after, components.clone());
        let diff = serde_json::to_value(compare_paths(&base, &next).unwrap()).unwrap();
        let request = &diff["operations"][0]["request"];
        assert_eq!(request["compatibility"], "unknown", "{name}: {request:#}");
        for list in [
            "added_properties",
            "removed_properties",
            "changed_properties",
            "required_added",
            "required_removed",
        ] {
            assert_eq!(
                request[list],
                json!([]),
                "{name}: {list} derived: {request:#}"
            );
        }
        let changes = request["changes"].as_array().unwrap();
        assert!(
            changes
                .iter()
                .all(|c| c["kind"] == "unsupported_changed" && c["compatibility"] == "unknown"),
            "{name}: {changes:#?}"
        );
    }
}
