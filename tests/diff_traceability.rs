//! `compare` traceability: moved operations are paired, every pointer is an
//! RFC 6901 pointer that resolves in the contract, changed components list
//! the operations they affect, JSON contracts carry line numbers, and
//! imported oasdiff evidence keeps its level, operation and path.

use std::fs;
use std::path::{Path, PathBuf};

use contract_flightplan::diff::compare_paths;
use serde_json::{json, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-trace-{name}-{}-{}",
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

fn write_json(dir: &Path, name: &str, doc: &Value) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, serde_json::to_string_pretty(doc).unwrap()).unwrap();
    path
}

fn diff_of(source: &Path, destination: &Path) -> Value {
    serde_json::to_value(compare_paths(source, destination).unwrap()).unwrap()
}

fn identity(op: &Value) -> (String, String) {
    (
        op["method"].as_str().unwrap().to_string(),
        op["path"].as_str().unwrap().to_string(),
    )
}

#[test]
fn moved_operation_is_classified_not_added_removed() {
    let negatives = root().join("fixtures/negatives");
    let diff = diff_of(
        &negatives.join("moved-op.base.openapi.yaml"),
        &negatives.join("moved-op.next.openapi.yaml"),
    );
    assert_eq!(diff["added_operations"], json!([]), "{diff:#}");
    assert_eq!(diff["removed_operations"], json!([]), "{diff:#}");
    let moved = diff["moved_operations"]
        .as_array()
        .expect("moved_operations");
    assert_eq!(moved.len(), 1, "{diff:#}");
    assert_eq!(
        identity(&moved[0]["from"]),
        ("POST".into(), "/users".into())
    );
    assert_eq!(
        identity(&moved[0]["to"]),
        ("POST".into(), "/v2/users".into())
    );
    assert_eq!(moved[0]["from"]["operation_id"], "createUser");
    // The moved pair is still compared body by body.
    let op = diff["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|op| op["destination_identity"]["path"] == "/v2/users")
        .expect("the moved pair is diffed");
    assert_eq!(op["identity"]["path"], "/users");
    let changes = op["request"]["changes"].as_array().unwrap();
    assert!(
        changes
            .iter()
            .any(|c| c["kind"] == "property_added" && c["field"] == "nickname"),
        "{changes:#?}"
    );
}

/// Every string under a `pointer` or `location` key, anywhere in the value.
fn pointers(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "pointer" || key == "location" {
                    if let Some(text) = child.as_str() {
                        if text.starts_with('/') {
                            out.push(text.to_string());
                        }
                    }
                }
                pointers(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| pointers(i, out)),
        _ => {}
    }
}

#[test]
fn pointers_are_rfc6901_escaped() {
    let dir = scratch("pointers");
    let contract = |extra: bool| {
        let mut props = json!({
            "a/b": {"type": "string"},
            "t~ilde": {"oneOf": [{"type": "string"}, {"type": "integer"}]}
        });
        if extra {
            props["a/b"] = json!({"type": "integer"});
            props["t~ilde"] = json!({"oneOf": [{"type": "string"}]});
        }
        json!({
            "openapi": "3.0.3",
            "info": {"title": "Pointers", "version": "1"},
            "paths": {
                "/users/{id}/a~b": {
                    "put": {
                        "operationId": "putUser",
                        "requestBody": {"content": {"application/json": {
                            "schema": {"type": "object", "properties": props}
                        }}},
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        })
    };
    let (base_doc, next_doc) = (contract(false), contract(true));
    let base = write_json(&dir, "base.json", &base_doc);
    let next = write_json(&dir, "next.json", &next_doc);
    let diff = diff_of(&base, &next);
    let mut found = Vec::new();
    pointers(&diff, &mut found);
    assert!(
        found.iter().any(|p| p.contains("/properties/a~1b")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|p| p.contains("/properties/t~0ilde")),
        "{found:#?}"
    );
    for pointer in &found {
        assert!(
            pointer.starts_with("/paths/~1users~1{id}~1a~0b/put"),
            "unescaped path segment in {pointer}"
        );
        assert!(
            base_doc.pointer(pointer).is_some() || next_doc.pointer(pointer).is_some(),
            "{pointer} does not resolve in either contract"
        );
    }
}

#[test]
fn component_ref_graph_lists_affected_operations() {
    let dir = scratch("refgraph");
    let contract = |zip: &str, price: &str, tag: &str| {
        json!({
            "openapi": "3.0.3",
            "info": {"title": "Refs", "version": "1"},
            "paths": {
                "/users": {"post": {
                    "operationId": "createUser",
                    "requestBody": {"content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"home": {"$ref": "#/components/schemas/Address"}}
                    }}}},
                    "responses": {"200": {"description": "ok"}}
                }},
                "/users/{id}": {"get": {
                    "operationId": "getUser",
                    "responses": {"200": {"description": "ok", "content": {"application/json": {
                        "schema": {"$ref": "#/components/schemas/User"}
                    }}}}
                }},
                "/orders": {"post": {
                    "operationId": "createOrder",
                    "requestBody": {"content": {"application/json": {"schema": {
                        "$ref": "#/components/schemas/Order"
                    }}}},
                    "responses": {"200": {"description": "ok"}}
                }},
                "/tags": {"post": {
                    "operationId": "createTag",
                    "requestBody": {"content": {"application/json": {"schema": {
                        "$ref": "#/components/schemas/Tag"
                    }}}},
                    "responses": {"200": {"description": "ok"}}
                }}
            },
            "components": {"schemas": {
                "Address": {"type": "object", "properties": {"zip": {"type": zip}}},
                "User": {"type": "object", "properties": {
                    "address": {"$ref": "#/components/schemas/Address"}
                }},
                "Order": {"type": "object", "properties": {
                    "lines": {"type": "array", "items": {"$ref": "#/components/schemas/Line"}}
                }},
                "Line": {"type": "object", "properties": {"price": {"type": price}}},
                "Tag": {"type": "object", "properties": {"label": {"type": tag}}}
            }}
        })
    };
    let base = write_json(&dir, "base.json", &contract("string", "integer", "string"));
    let next = write_json(&dir, "next.json", &contract("integer", "number", "string"));
    let diff = diff_of(&base, &next);
    let affected = diff["affected_schemas"]
        .as_array()
        .expect("affected_schemas");
    let listed: Vec<(&str, Vec<(String, String)>)> = affected
        .iter()
        .map(|a| {
            (
                a["component_pointer"].as_str().unwrap(),
                a["operations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(identity)
                    .collect(),
            )
        })
        .collect();
    let op = |m: &str, p: &str| (m.to_string(), p.to_string());
    assert_eq!(
        listed,
        vec![
            (
                "/components/schemas/Address",
                vec![op("GET", "/users/{id}"), op("POST", "/users")]
            ),
            ("/components/schemas/Line", vec![op("POST", "/orders")]),
        ],
        "only changed components, each with every operation that reaches it: {affected:#?}"
    );
}

/// 1-based line of the first line containing `needle`.
fn line_of(text: &str, needle: &str) -> u64 {
    text.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle} not in text")) as u64
        + 1
}

#[test]
fn json_contract_changes_carry_lines_and_yaml_says_line_unavailable() {
    let dir = scratch("lines");
    let contract = |zip: &str| {
        json!({
            "openapi": "3.0.3",
            "info": {"title": "Lines", "version": "1"},
            "paths": {"/users": {"post": {
                "operationId": "createUser",
                "requestBody": {"content": {"application/json": {"schema": {
                    "type": "object",
                    "properties": {
                        "nameField": {"type": zip},
                        "home": {"$ref": "#/components/schemas/Address"}
                    }
                }}}},
                "responses": {"200": {"description": "ok"}}
            }}},
            "components": {"schemas": {
                "Address": {"type": "object", "properties": {"zipField": {"type": zip}}}
            }}
        })
    };
    let base = write_json(&dir, "base.json", &contract("string"));
    let next = write_json(&dir, "next.json", &contract("integer"));
    let base_text = fs::read_to_string(&base).unwrap();
    let next_text = fs::read_to_string(&next).unwrap();
    let diff = diff_of(&base, &next);
    assert_eq!(diff["source"]["line_numbers"], "json_key_path");
    let changes = diff["operations"][0]["request"]["changes"]
        .as_array()
        .unwrap();
    let at = |field: &str| {
        changes
            .iter()
            .find(|c| c["field"] == field)
            .unwrap_or_else(|| panic!("no change for {field}: {changes:#?}"))
    };
    let name = at("nameField");
    assert_eq!(name["source_line"], line_of(&base_text, "\"nameField\""));
    assert_eq!(
        name["destination_line"],
        line_of(&next_text, "\"nameField\"")
    );
    // Through a $ref, the line is where the component's property is written.
    let zip = at("home.zipField");
    assert_eq!(zip["source_line"], line_of(&base_text, "\"zipField\""));

    let pack = root().join("packs/rename-user-id");
    let diff = diff_of(
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
    );
    assert_eq!(diff["source"]["line_numbers"], "line_unavailable");
    assert_eq!(diff["destination"]["line_numbers"], "line_unavailable");
    let changes = diff["operations"][0]["request"]["changes"]
        .as_array()
        .unwrap();
    assert!(!changes.is_empty());
    for change in changes {
        assert!(change["source_line"].is_null(), "{change:#}");
        assert!(change["destination_line"].is_null(), "{change:#}");
    }
}

#[test]
fn oasdiff_array_shape_is_ingested_with_level() {
    let evidence = contract_flightplan::oasdiff::ingest_oasdiff(
        &root().join("fixtures/negatives/oasdiff-array.json"),
    )
    .unwrap();
    let first = serde_json::to_value(&evidence[0]).unwrap();
    assert_eq!(first["identity"], "request-body-changed");
    assert_eq!(first["level"], 3);
    assert_eq!(first["operation"], "POST");
    assert_eq!(first["path"], "/users");
    assert_eq!(first["compatibility"], "breaking");

    let levels = contract_flightplan::oasdiff::ingest_oasdiff_value(&json!([
        {"id": "a", "text": "warn", "level": 2},
        {"id": "b", "text": "info", "level": 1},
        {"id": "c", "text": "none"}
    ]))
    .unwrap();
    let classes: Vec<Value> = levels
        .iter()
        .map(|e| serde_json::to_value(e).unwrap()["compatibility"].clone())
        .collect();
    assert_eq!(
        classes,
        vec![json!("unknown"), json!("non_breaking"), json!("unknown")]
    );
}

#[test]
fn html_report_lists_moved_operations_and_affected_schemas() {
    let negatives = root().join("fixtures/negatives");
    let report = contract_flightplan::report::build_report(
        &negatives.join("moved-op.base.openapi.yaml"),
        &negatives.join("moved-op.next.openapi.yaml"),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let html = contract_flightplan::report::render_html(&report);
    assert!(
        html.contains("Moved: POST /users → POST /v2/users"),
        "{html}"
    );
    assert!(
        html.contains("Request compatibility: non_breaking"),
        "{html}"
    );

    let dir = scratch("report-affected");
    let contract = |zip: &str| {
        json!({
            "openapi": "3.0.3",
            "info": {"title": "Refs", "version": "1"},
            "paths": {"/users": {"post": {
                "operationId": "createUser",
                "requestBody": {"content": {"application/json": {"schema": {
                    "$ref": "#/components/schemas/Address"
                }}}},
                "responses": {"200": {"description": "ok"}}
            }}},
            "components": {"schemas": {
                "Address": {"type": "object", "properties": {"zip": {"type": zip}}}
            }}
        })
    };
    let base = write_json(&dir, "base.json", &contract("string"));
    let next = write_json(&dir, "next.json", &contract("integer"));
    let report =
        contract_flightplan::report::build_report(&base, &next, None, None, None, None).unwrap();
    let html = contract_flightplan::report::render_html(&report);
    assert!(
        html.contains("Affected component /components/schemas/Address: POST /users"),
        "{html}"
    );
}
