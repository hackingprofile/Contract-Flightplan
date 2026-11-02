//! `compare` diffs request and response bodies at every depth reachable
//! through `properties` and `items`, gives every change an RFC 6901 pointer and
//! a compatibility class, and never reports a changed schema as "no change".

use std::fs;
use std::path::{Path, PathBuf};

use contract_flightplan::diff::compare_paths;
use serde_json::{json, Value};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-diff-{name}-{}-{}",
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

/// A JSON contract with `POST /users` whose request body and 200 response use
/// the given schemas, plus optional component schemas.
fn contract(dir: &Path, name: &str, request: Value, response: Value, components: Value) -> PathBuf {
    let doc = json!({
        "openapi": "3.0.3",
        "info": {"title": "Diff depth", "version": "1"},
        "paths": {
            "/users": {
                "post": {
                    "operationId": "createUser",
                    "requestBody": {"content": {"application/json": {"schema": request}}},
                    "responses": {
                        "200": {
                            "description": "ok",
                            "content": {"application/json": {"schema": response}}
                        }
                    }
                }
            }
        },
        "components": {"schemas": components}
    });
    let path = dir.join(name);
    fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    path
}

fn object(properties: Value, required: Value) -> Value {
    json!({"type": "object", "required": required, "properties": properties})
}

fn diff_value(dir: &Path, request: (Value, Value), response: (Value, Value)) -> Value {
    diff_value_with(dir, request, response, json!({}), json!({}))
}

fn diff_value_with(
    dir: &Path,
    request: (Value, Value),
    response: (Value, Value),
    base_components: Value,
    next_components: Value,
) -> Value {
    let base = contract(dir, "base.json", request.0, response.0, base_components);
    let next = contract(dir, "next.json", request.1, response.1, next_components);
    serde_json::to_value(compare_paths(&base, &next).unwrap()).unwrap()
}

fn body<'a>(diff: &'a Value, which: &str) -> &'a Value {
    let op = &diff["operations"][0];
    match which {
        "request" => &op["request"],
        status => &op["responses"][status],
    }
}

fn changes(body: &Value) -> Vec<Value> {
    body["changes"].as_array().cloned().unwrap_or_default()
}

fn change_at<'a>(changes: &'a [Value], suffix: &str) -> Vec<&'a Value> {
    changes
        .iter()
        .filter(|c| c["pointer"].as_str().is_some_and(|p| p.ends_with(suffix)))
        .collect()
}

const REQUEST_SCHEMA: &str = "/paths/~1users/post/requestBody/content/application~1json/schema";

#[test]
fn nested_property_type_change_is_detected_with_pointer() {
    let dir = scratch("nested");
    let address = |zip: &str| {
        object(
            json!({"address": object(json!({"zip": {"type": zip}}), json!([]))}),
            json!([]),
        )
    };
    let flat = object(json!({"name": {"type": "string"}}), json!([]));
    let diff = diff_value(
        &dir,
        (address("string"), address("integer")),
        (flat.clone(), flat),
    );
    let request = body(&diff, "request");
    let all = changes(request);
    let zip = change_at(&all, "/properties/address/properties/zip");
    assert_eq!(zip.len(), 1, "exactly one change for zip: {all:#?}");
    assert_eq!(
        zip[0]["pointer"],
        format!("{REQUEST_SCHEMA}/properties/address/properties/zip")
    );
    assert_eq!(zip[0]["kind"], "type_changed");
    assert_eq!(zip[0]["compatibility"], "breaking");
    assert_eq!(zip[0]["field"], "address.zip");
    assert_eq!(request["compatibility"], "breaking");
    assert_eq!(body(&diff, "200")["compatibility"], "unchanged");
}

#[test]
fn required_added_is_breaking_and_optional_added_is_non_breaking() {
    let dir = scratch("required");
    let before = object(json!({"name": {"type": "string"}}), json!(["name"]));
    let after = object(
        json!({
            "name": {"type": "string"},
            "nickname": {"type": "string"},
            "email": {"type": "string"}
        }),
        json!(["name", "email"]),
    );
    let diff = diff_value(&dir, (before.clone(), after), (before.clone(), before));
    let all = changes(body(&diff, "request"));
    let nickname = change_at(&all, "/properties/nickname");
    assert_eq!(nickname.len(), 1, "{all:#?}");
    assert_eq!(nickname[0]["kind"], "property_added");
    assert_eq!(nickname[0]["compatibility"], "non_breaking");
    let email = change_at(&all, "/properties/email");
    assert_eq!(email.len(), 1, "{all:#?}");
    assert_eq!(email[0]["kind"], "property_added");
    assert_eq!(email[0]["compatibility"], "breaking");

    // An existing property that becomes required is breaking in a request.
    let dir = scratch("required-existing");
    let optional = object(json!({"name": {"type": "string"}}), json!([]));
    let required = object(json!({"name": {"type": "string"}}), json!(["name"]));
    let diff = diff_value(
        &dir,
        (optional.clone(), required),
        (optional.clone(), optional),
    );
    let all = changes(body(&diff, "request"));
    let name = change_at(&all, "/properties/name");
    assert_eq!(name.len(), 1, "{all:#?}");
    assert_eq!(name[0]["kind"], "required_added");
    assert_eq!(name[0]["compatibility"], "breaking");
}

#[test]
fn unsupported_construct_yields_unknown_compatibility() {
    // A flat object becomes an allOf wrapper: the body is unknown and no
    // removals are derived from the unsupported side.
    let dir = scratch("allof");
    let flat = object(
        json!({"a": {"type": "string"}, "b": {"type": "string"}}),
        json!([]),
    );
    let wrapped = json!({"allOf": [{"$ref": "#/components/schemas/Base"}]});
    let components = json!({"Base": flat.clone()});
    let diff = diff_value_with(
        &dir,
        (flat.clone(), wrapped),
        (flat.clone(), flat.clone()),
        json!({}),
        components,
    );
    let request = body(&diff, "request");
    assert_eq!(request["compatibility"], "unknown", "{request:#}");
    let all = changes(request);
    assert!(
        all.iter().all(|c| c["kind"] != "property_removed"),
        "{all:#?}"
    );
    let root = change_at(&all, "/schema");
    assert_eq!(root.len(), 1, "{all:#?}");
    assert_eq!(root[0]["compatibility"], "unknown");

    // The same holds for a nested property that uses a combinator.
    let dir = scratch("nested-oneof");
    let plain = object(json!({"meta": {"type": "string"}}), json!([]));
    let one_of = object(
        json!({"meta": {"oneOf": [{"type": "string"}, {"type": "integer"}]}}),
        json!([]),
    );
    let diff = diff_value(&dir, (plain.clone(), one_of), (plain.clone(), plain));
    let request = body(&diff, "request");
    let all = changes(request);
    let meta = change_at(&all, "/properties/meta");
    assert_eq!(meta.len(), 1, "{all:#?}");
    assert_eq!(meta[0]["compatibility"], "unknown");
    assert_eq!(request["compatibility"], "unknown");
}

#[test]
fn unclassified_nested_keyword_change_is_unknown_not_silent() {
    let dir = scratch("keyword");
    let with_max = |max: u64| {
        object(
            json!({"address": object(json!({"zip": {"type": "string", "maxLength": max}}), json!([]))}),
            json!([]),
        )
    };
    let flat = object(json!({"name": {"type": "string"}}), json!([]));
    let diff = diff_value(&dir, (with_max(5), with_max(3)), (flat.clone(), flat));
    let request = body(&diff, "request");
    let all = changes(request);
    let zip = change_at(&all, "/properties/address/properties/zip");
    assert_eq!(zip.len(), 1, "{all:#?}");
    assert_eq!(zip[0]["kind"], "keyword_changed");
    assert_eq!(zip[0]["compatibility"], "unknown");
    assert!(zip[0]["reason"].as_str().unwrap().contains("maxLength"));
    assert_eq!(request["compatibility"], "unknown");

    // Documentation-only changes are reported, never classified as breaking.
    let dir = scratch("annotation");
    let described = |text: &str| {
        object(
            json!({"name": {"type": "string", "description": text}}),
            json!([]),
        )
    };
    let flat = object(json!({"name": {"type": "string"}}), json!([]));
    let diff = diff_value(
        &dir,
        (described("old"), described("new")),
        (flat.clone(), flat),
    );
    let all = changes(body(&diff, "request"));
    let name = change_at(&all, "/properties/name");
    assert_eq!(name.len(), 1, "{all:#?}");
    assert_eq!(name[0]["kind"], "annotation_changed");
    assert_eq!(name[0]["compatibility"], "non_breaking");
}

#[test]
fn widening_is_non_breaking_in_a_request_and_breaking_in_a_response() {
    let dir = scratch("widen");
    let strict = object(json!({"note": {"type": "string"}}), json!([]));
    let nullable = object(
        json!({"note": {"type": "string", "nullable": true}}),
        json!([]),
    );
    let diff = diff_value(&dir, (strict.clone(), nullable.clone()), (strict, nullable));
    let request = changes(body(&diff, "request"));
    let response = changes(body(&diff, "200"));
    let req_note = change_at(&request, "/properties/note");
    let resp_note = change_at(&response, "/properties/note");
    assert_eq!(req_note.len(), 1, "{request:#?}");
    assert_eq!(resp_note.len(), 1, "{response:#?}");
    assert_eq!(req_note[0]["kind"], "type_changed");
    assert_eq!(req_note[0]["compatibility"], "non_breaking");
    assert_eq!(resp_note[0]["compatibility"], "breaking");
}

#[test]
fn enum_shrunk_is_breaking_and_enum_grown_is_non_breaking() {
    let dir = scratch("enum");
    let colors = |values: Value| {
        object(
            json!({"color": {"type": "string", "enum": values}}),
            json!([]),
        )
    };
    let flat = object(json!({"name": {"type": "string"}}), json!([]));
    let diff = diff_value(
        &dir,
        (colors(json!(["red", "blue"])), colors(json!(["red"]))),
        (flat.clone(), flat.clone()),
    );
    let all = changes(body(&diff, "request"));
    let color = change_at(&all, "/properties/color");
    assert_eq!(color.len(), 1, "{all:#?}");
    assert_eq!(color[0]["kind"], "enum_changed");
    assert_eq!(color[0]["compatibility"], "breaking");

    let dir = scratch("enum-grow");
    let diff = diff_value(
        &dir,
        (colors(json!(["red"])), colors(json!(["red", "blue"]))),
        (flat.clone(), flat),
    );
    let all = changes(body(&diff, "request"));
    let color = change_at(&all, "/properties/color");
    assert_eq!(color.len(), 1, "{all:#?}");
    assert_eq!(color[0]["compatibility"], "non_breaking");
}

#[test]
fn nested_changes_inside_array_items_and_refs_are_reported() {
    let dir = scratch("items");
    let order = object(
        json!({"lines": {"type": "array", "items": {"$ref": "#/components/schemas/Line"}}}),
        json!([]),
    );
    let line = |ty: &str| object(json!({"price": {"type": ty}}), json!([]));
    let flat = object(json!({"name": {"type": "string"}}), json!([]));
    let diff = diff_value_with(
        &dir,
        (order.clone(), order),
        (flat.clone(), flat),
        json!({"Line": line("integer")}),
        json!({"Line": line("string")}),
    );
    let all = changes(body(&diff, "request"));
    let price = change_at(&all, "/properties/lines/items/properties/price");
    assert_eq!(price.len(), 1, "{all:#?}");
    assert_eq!(price[0]["field"], "lines[].price");
    assert_eq!(price[0]["compatibility"], "breaking");
}

#[test]
fn diff_is_identical_across_two_runs() {
    let dir = scratch("determinism");
    let rich = |zip: &str, extra: bool| {
        let mut props = json!({
            "address": object(json!({"zip": {"type": zip}, "city": {"type": "string"}}), json!(["city"])),
            "tags": {"type": "array", "items": {"type": "string", "enum": ["a", "b"]}}
        });
        if extra {
            props["nickname"] = json!({"type": "string"});
        }
        object(props, json!(["address"]))
    };
    let base = contract(
        &dir,
        "base.json",
        rich("string", false),
        rich("string", false),
        json!({}),
    );
    let next = contract(
        &dir,
        "next.json",
        rich("integer", true),
        rich("string", true),
        json!({}),
    );
    let first = serde_json::to_string(&compare_paths(&base, &next).unwrap()).unwrap();
    let second = serde_json::to_string(&compare_paths(&base, &next).unwrap()).unwrap();
    assert_eq!(first, second);
    let value: Value = serde_json::from_str(&first).unwrap();
    let request = changes(body(&value, "request"));
    let response = changes(body(&value, "200"));
    assert!(
        request.len() >= 2 && !response.is_empty(),
        "the classified changes are part of the deterministic output: {value:#}"
    );
}

/// Adding `type` to an untyped property narrows it from any type; the change
/// names "any", never a made-up "object".
#[test]
fn typing_an_untyped_property_is_a_narrowing_from_any() {
    let dir = std::env::temp_dir().join(format!("cfp-untyped-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let doc = |status: serde_json::Value| {
        serde_json::json!({
            "openapi": "3.0.3",
            "info": {"title": "T", "version": "1"},
            "paths": {"/x": {"post": {
                "operationId": "x",
                "requestBody": {"content": {"application/json": {"schema": {
                    "type": "object",
                    "properties": {"status": status}
                }}}},
                "responses": {"200": {"description": "ok"}}
            }}}
        })
    };
    let base = dir.join("base.json");
    let next = dir.join("next.json");
    std::fs::write(
        &base,
        doc(serde_json::json!({"enum": ["a", "b"]})).to_string(),
    )
    .unwrap();
    std::fs::write(
        &next,
        doc(serde_json::json!({"type": "string", "enum": ["a", "b"]})).to_string(),
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cfp"))
        .args(["compare", "--source"])
        .arg(&base)
        .arg("--destination")
        .arg(&next)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("type any -> string"), "{text}");
    assert!(!text.contains("object -> string"), "{text}");
}

/// Compare a pair of hand-written JSON contracts and return the diff document.
fn compare_docs(name: &str, base: serde_json::Value, next: serde_json::Value) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("cfp-params-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (b, n) = (dir.join("base.json"), dir.join("next.json"));
    std::fs::write(&b, base.to_string()).unwrap();
    std::fs::write(&n, next.to_string()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cfp"))
        .args(["compare", "--source"])
        .arg(&b)
        .arg("--destination")
        .arg(&n)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn contract_with_parameters(params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "openapi": "3.0.3",
        "info": {"title": "T", "version": "1"},
        "paths": {"/items/{id}": {
            "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
            "get": {
                "operationId": "getItem",
                "parameters": params,
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object"}}}}}
            }
        }}
    })
}

/// A new required query, header or path parameter breaks callers; the
/// operation must not read as unchanged. Parameters stay unmapped.
#[test]
fn parameter_additions_removals_and_required_changes_are_classified() {
    let base = contract_with_parameters(serde_json::json!([
        {"name": "limit", "in": "query", "required": false, "schema": {"type": "integer"}},
        {"name": "trace", "in": "header", "schema": {"type": "string"}},
        {"name": "old", "in": "query", "schema": {"type": "string"}}
    ]));
    let next = contract_with_parameters(serde_json::json!([
        {"name": "limit", "in": "query", "required": true, "schema": {"type": "integer"}},
        {"name": "trace", "in": "header", "schema": {"type": "string"}},
        {"name": "tenant", "in": "header", "required": true, "schema": {"type": "string"}},
        {"name": "page", "in": "query", "schema": {"type": "integer"}}
    ]));
    let diff = compare_docs("classes", base, next);
    let op = &diff["operations"][0];
    assert_eq!(op["compatibility"], "breaking", "{op:#}");
    let changes = op["parameter_changes"]
        .as_array()
        .expect("parameter_changes");
    let find = |name: &str| {
        changes
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no change for {name}: {changes:#?}"))
    };
    assert_eq!(find("limit")["kind"], "required_added");
    assert_eq!(find("limit")["in"], "query");
    assert_eq!(find("limit")["compatibility"], "breaking");
    assert_eq!(find("tenant")["kind"], "added");
    assert_eq!(find("tenant")["compatibility"], "breaking");
    assert_eq!(find("page")["kind"], "added");
    assert_eq!(find("page")["compatibility"], "non_breaking");
    assert_eq!(find("old")["kind"], "removed");
    assert_eq!(find("old")["compatibility"], "unknown");
    assert!(changes
        .iter()
        .all(|c| c["name"] != "trace" && c["name"] != "id"));
    // The document still matches the published diff schema.
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/v1/diff.schema.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&diff)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    // An operation whose parameters did not change stays unchanged.
    let same = contract_with_parameters(serde_json::json!([]));
    let diff = compare_docs("same", same.clone(), same);
    assert_eq!(diff["operations"][0]["compatibility"], "unchanged");
}
