use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::digest::{
    check_depth, read_bounded_file, utf8_text, MAX_DEPTH, MAX_PROPERTIES, MAX_REF_CHAIN,
};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    Openapi30,
    Openapi31,
}

#[derive(Debug, Clone)]
pub struct OpenApiDoc {
    pub path: PathBuf,
    pub digest: String,
    pub dialect: Dialect,
    pub openapi_version: String,
    pub title: String,
    pub api_version: String,
    pub value: Value,
    /// Line of every RFC 6901 pointer in a JSON contract (the line of an
    /// object member's key, or of an array element). `None` for YAML, whose
    /// positions this tool does not track.
    pub line_index: Option<BTreeMap<String, usize>>,
}

impl OpenApiDoc {
    /// The 1-based line of `pointer` in the contract file, when known.
    pub fn line_of(&self, pointer: &str) -> Option<usize> {
        self.line_index.as_ref()?.get(pointer).copied()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OpIdentity {
    pub method: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OperationView {
    pub identity: OpIdentity,
    pub pointer: String,
    pub request: Option<BodyView>,
    pub responses: BTreeMap<String, BodyView>,
    pub notes: Vec<String>,
    /// Path, query, header and cookie parameters: the path item's, overridden
    /// by the operation's with the same name and location.
    pub parameters: Vec<ParamView>,
}

/// One declared parameter. Parameters are compared, never mapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamView {
    pub name: String,
    /// `path`, `query`, `header` or `cookie`.
    pub location: String,
    pub required: bool,
}

#[derive(Debug, Clone)]
pub struct BodyView {
    pub content_type: String,
    pub pointer: String,
    pub schema: Value,
    pub resolved: crate::schema::ResolvedSchema,
}

/// How a contract or fixture path is recorded in outputs.
///
/// Relative paths are kept as given (with `/` separators) because they carry
/// repository context. Absolute paths are reduced to the file name so that
/// reports, migrations, and SARIF never disclose a user's directory layout.
pub fn portable_path(path: &Path) -> String {
    if path.is_absolute() {
        return path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
    }
    path.display().to_string().replace('\\', "/")
}

pub fn load_openapi(path: &Path) -> Result<OpenApiDoc> {
    let bytes = read_bounded_file(path)?;
    // The digest of the bytes parsed here, not of a second read.
    let digest = crate::digest::sha256_prefixed(&bytes);
    let text = utf8_text(path, &bytes)?;
    let value = parse_document(path, &text)?;
    let line_index = is_json(path, &text).then(|| json_line_index(&text));
    check_depth(&value, &path.display().to_string())?;
    scan_refs(&value, "$")?;
    let openapi_version = value
        .get("openapi")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Parse {
            path: path.display().to_string(),
            message: "missing string 'openapi' version field".into(),
        })?
        .to_string();
    let dialect = if openapi_version.starts_with("3.0.") || openapi_version == "3.0" {
        Dialect::Openapi30
    } else if openapi_version.starts_with("3.1.") || openapi_version == "3.1" {
        Dialect::Openapi31
    } else {
        return Err(Error::Unsupported {
            construct: "openapi_version".into(),
            location: "/openapi".into(),
            message: format!(
                "only OpenAPI 3.0 and 3.1 are in the supported subset; got {openapi_version}"
            ),
        });
    };
    let info = value
        .get("info")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Parse {
            path: path.display().to_string(),
            message: "missing object 'info'".into(),
        })?;
    let title = info
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("untitled")
        .to_string();
    let api_version = info
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("0")
        .to_string();
    if !value.get("paths").map(Value::is_object).unwrap_or(false) {
        return Err(Error::Parse {
            path: path.display().to_string(),
            message: "missing object 'paths'".into(),
        });
    }
    Ok(OpenApiDoc {
        path: path.to_path_buf(),
        digest,
        dialect,
        openapi_version,
        title,
        api_version,
        value,
        line_index,
    })
}

fn is_json(path: &Path, text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[') || extension_is(path, "json")
}

/// Map every RFC 6901 pointer in a JSON text to the 1-based line where it
/// starts: the member's key for an object member, the value for an array
/// element. The text must already have parsed as JSON.
pub fn json_line_index(text: &str) -> BTreeMap<String, usize> {
    let mut scanner = LineScanner {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        line: 1,
        index: BTreeMap::new(),
    };
    scanner.space();
    scanner.index.insert(String::new(), scanner.line);
    scanner.value("");
    scanner.index
}

struct LineScanner<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    index: BTreeMap<String, usize>,
}

impl LineScanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn space(&mut self) {
        while let Some(b) = self.peek() {
            match b {
                b'\n' => self.line += 1,
                b' ' | b'\t' | b'\r' => {}
                _ => break,
            }
            self.pos += 1;
        }
    }

    /// Read a string token starting at its opening quote.
    fn string(&mut self) -> String {
        let start = self.pos;
        self.pos += 1;
        while let Some(b) = self.peek() {
            self.pos += if b == b'\\' { 2 } else { 1 };
            if b == b'"' {
                break;
            }
        }
        let end = self.pos.min(self.bytes.len());
        serde_json::from_str(&self.text[start..end]).unwrap_or_default()
    }

    fn value(&mut self, pointer: &str) {
        self.space();
        match self.peek() {
            Some(b'{') => {
                self.pos += 1;
                loop {
                    self.space();
                    match self.peek() {
                        None => break,
                        Some(b'}') => {
                            self.pos += 1;
                            break;
                        }
                        Some(b',') => self.pos += 1,
                        Some(b'"') => {
                            let line = self.line;
                            let key = self.string();
                            let child = format!("{pointer}/{}", pointer_escape(&key));
                            self.index.entry(child.clone()).or_insert(line);
                            self.space();
                            if self.peek() == Some(b':') {
                                self.pos += 1;
                            }
                            self.value(&child);
                        }
                        Some(_) => self.pos += 1,
                    }
                }
            }
            Some(b'[') => {
                self.pos += 1;
                let mut i = 0;
                loop {
                    self.space();
                    match self.peek() {
                        None => break,
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        Some(b',') => self.pos += 1,
                        Some(_) => {
                            let child = format!("{pointer}/{i}");
                            self.index.entry(child.clone()).or_insert(self.line);
                            self.value(&child);
                            i += 1;
                        }
                    }
                }
            }
            Some(b'"') => {
                self.string();
            }
            Some(_) => {
                while let Some(b) = self.peek() {
                    if matches!(b, b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                        break;
                    }
                    self.pos += 1;
                }
            }
            None => {}
        }
    }
}

pub fn parse_document(path: &Path, text: &str) -> Result<Value> {
    let trimmed = text.trim_start();
    let looks_json = trimmed.starts_with('{') || trimmed.starts_with('[');
    if looks_json || extension_is(path, "json") {
        crate::strict_json::from_str(text).map_err(|e| Error::Parse {
            path: path.display().to_string(),
            message: format!("JSON: {e}"),
        })
    } else {
        if let Some(line) = crate::yaml_nesting::first_line_deeper_than(text, MAX_DEPTH) {
            return Err(Error::Bound {
                kind: "depth".into(),
                location: path.display().to_string(),
                message: format!(
                    "collections nest deeper than {MAX_DEPTH} levels at line {line}; refused before parsing"
                ),
            });
        }
        crate::strict_json::from_yaml_str(text).map_err(|e| Error::Parse {
            path: path.display().to_string(),
            message: format!("YAML: {e}"),
        })
    }
}

fn extension_is(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}

pub fn validate_local_ref(reference: &str, location: &str) -> Result<()> {
    let lower = reference.to_ascii_lowercase();
    if lower.contains("://")
        || lower.starts_with("file:")
        || lower.starts_with("http:")
        || lower.starts_with("https:")
        || reference.starts_with("//")
    {
        return Err(Error::Unsupported {
            construct: "remote_reference".into(),
            location: location.into(),
            message: format!("remote or protocol reference is rejected: {reference}"),
        });
    }
    if !reference.starts_with("#/") {
        return Err(Error::Unsupported {
            construct: "reference".into(),
            location: location.into(),
            message: format!("only local JSON pointers starting with #/ are allowed: {reference}"),
        });
    }
    if reference.contains("..") {
        return Err(Error::Unsupported {
            construct: "reference_traversal".into(),
            location: location.into(),
            message: format!("parent traversal is rejected: {reference}"),
        });
    }
    for part in reference.trim_start_matches("#/").split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(Error::Unsupported {
                construct: "reference_traversal".into(),
                location: location.into(),
                message: format!("empty or traversal pointer segment in {reference}"),
            });
        }
    }
    Ok(())
}

fn scan_refs(value: &Value, location: &str) -> Result<()> {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                scan_refs(item, &format!("{location}/{i}"))?;
            }
        }
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get("$ref") {
                validate_local_ref(r, location)?;
            } else if map.get("$ref").is_some() {
                return Err(Error::Parse {
                    path: location.into(),
                    message: "$ref must be a string".into(),
                });
            }
            for (k, v) in map {
                scan_refs(v, &format!("{location}/{k}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Escape one JSON Pointer reference token (RFC 6901 section 3).
pub fn pointer_escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

pub fn pointer_unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

pub fn resolve_pointer<'a>(root: &'a Value, pointer: &str) -> Result<&'a Value> {
    if pointer == "#" || pointer.is_empty() {
        return Ok(root);
    }
    let trimmed = pointer.strip_prefix("#/").ok_or_else(|| Error::Invalid {
        message: format!("expected local pointer, got {pointer}"),
    })?;
    let mut current = root;
    for raw in trimmed.split('/') {
        let key = pointer_unescape(raw);
        current = match current {
            Value::Object(map) => map.get(&key).ok_or_else(|| Error::Invalid {
                message: format!("pointer {pointer} missing object key {key}"),
            })?,
            Value::Array(items) => {
                let idx: usize = key.parse().map_err(|_| Error::Invalid {
                    message: format!("pointer {pointer} has non-index {key}"),
                })?;
                items.get(idx).ok_or_else(|| Error::Invalid {
                    message: format!("pointer {pointer} index {idx} out of range"),
                })?
            }
            _ => {
                return Err(Error::Invalid {
                    message: format!("pointer {pointer} walked into a non-container"),
                });
            }
        };
    }
    Ok(current)
}

pub fn resolve_ref<'a>(
    root: &'a Value,
    reference: &str,
    stack: &mut Vec<String>,
) -> Result<&'a Value> {
    validate_local_ref(reference, reference)?;
    if stack.iter().any(|s| s == reference) {
        return Err(Error::Unsupported {
            construct: "recursive_reference".into(),
            location: reference.into(),
            message: format!("cycle: {} -> {reference}", stack.join(" -> ")),
        });
    }
    if stack.len() >= MAX_REF_CHAIN {
        return Err(Error::Bound {
            kind: "ref_chain".into(),
            location: reference.into(),
            message: format!("$ref chain exceeds {MAX_REF_CHAIN}"),
        });
    }
    stack.push(reference.to_string());
    let target = resolve_pointer(root, reference)?;
    if let Some(Value::String(nested)) = target.get("$ref") {
        if target.as_object().map(|m| m.len() > 1).unwrap_or(false) {
            return Err(Error::Unsupported {
                construct: "ref_siblings".into(),
                location: reference.into(),
                message: "keywords beside $ref are outside the supported subset".into(),
            });
        }
        return resolve_ref(root, nested, stack);
    }
    Ok(target)
}

pub fn operations(doc: &OpenApiDoc) -> Result<Vec<OperationView>> {
    let paths = doc
        .value
        .get("paths")
        .and_then(Value::as_object)
        .expect("checked on load");
    let mut out = Vec::new();
    // One budget for every body: the bound is per contract, not per body.
    let mut budget = crate::schema::ResolveBudget::default();
    let methods = [
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];
    for (path, item) in paths {
        if path.starts_with("x-") {
            continue;
        }
        if item.get("$ref").is_some() {
            return Err(Error::Unsupported {
                construct: "path_item_ref".into(),
                location: format!("/paths/{}", pointer_escape(path)),
                message: "path item $ref is outside the supported subset".into(),
            });
        }
        let Some(item_obj) = item.as_object() else {
            return Err(Error::Parse {
                path: doc.path.display().to_string(),
                message: format!("path item {path} is not an object"),
            });
        };
        for method in methods {
            let Some(op) = item_obj.get(method) else {
                continue;
            };
            let Some(op_obj) = op.as_object() else {
                return Err(Error::Parse {
                    path: doc.path.display().to_string(),
                    message: format!("{method} {path} is not an object"),
                });
            };
            let pointer = format!("/paths/{}/{method}", pointer_escape(path));
            let operation_id = op_obj
                .get("operationId")
                .and_then(Value::as_str)
                .map(str::to_string);
            let mut notes = Vec::new();
            if op_obj.get("parameters").is_some() || item_obj.get("parameters").is_some() {
                notes.push(
                    "path/query/header parameters are not mapped in the supported subset; they are not transformed"
                        .into(),
                );
            }
            let request = match op_obj.get("requestBody") {
                Some(body) => Some(extract_json_body(
                    doc,
                    body,
                    &format!("{pointer}/requestBody"),
                    &mut budget,
                )?),
                None => None,
            };
            let mut responses = BTreeMap::new();
            if let Some(Value::Object(resp_map)) = op_obj.get("responses") {
                for (status, resp) in resp_map {
                    if status.starts_with("x-") {
                        continue;
                    }
                    if resp.get("content").is_some() {
                        let body = extract_json_body(
                            doc,
                            resp,
                            &format!("{pointer}/responses/{}", pointer_escape(status)),
                            &mut budget,
                        )?;
                        responses.insert(status.clone(), body);
                    }
                }
            }
            let parameters = collect_parameters(doc, item_obj, op_obj, &pointer)?;
            out.push(OperationView {
                identity: OpIdentity {
                    method: method.to_ascii_uppercase(),
                    path: path.clone(),
                    operation_id,
                },
                pointer,
                request,
                responses,
                notes,
                parameters,
            });
        }
    }
    check_ambiguous_operation_ids(&out)?;
    Ok(out)
}

/// The operation's parameters: path-item parameters first, each replaced by an
/// operation parameter with the same name and location. A local `$ref` is
/// followed; a path parameter is always required.
fn collect_parameters(
    doc: &OpenApiDoc,
    item: &Map<String, Value>,
    op: &Map<String, Value>,
    pointer: &str,
) -> Result<Vec<ParamView>> {
    let mut params: Vec<ParamView> = Vec::new();
    for (list, at) in [
        (item.get("parameters"), "path item"),
        (op.get("parameters"), "operation"),
    ] {
        let Some(list) = list else { continue };
        let Some(items) = list.as_array() else {
            return Err(Error::Parse {
                path: doc.path.display().to_string(),
                message: format!("{at} parameters at {pointer} are not an array"),
            });
        };
        for raw in items {
            let param = match raw.get("$ref").and_then(Value::as_str) {
                Some(reference) => {
                    validate_local_ref(reference, pointer)?;
                    resolve_pointer(&doc.value, reference)?
                }
                None => raw,
            };
            let (Some(name), Some(location)) = (
                param.get("name").and_then(Value::as_str),
                param.get("in").and_then(Value::as_str),
            ) else {
                return Err(Error::Parse {
                    path: doc.path.display().to_string(),
                    message: format!("a parameter at {pointer} lacks a string name or in"),
                });
            };
            if !["path", "query", "header", "cookie"].contains(&location) {
                return Err(Error::Parse {
                    path: doc.path.display().to_string(),
                    message: format!(
                        "parameter {name} at {pointer} has in: {location}; OpenAPI 3 allows path, query, header or cookie"
                    ),
                });
            }
            let required = location == "path"
                || param
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            let view = ParamView {
                name: name.to_string(),
                location: location.to_string(),
                required,
            };
            match params
                .iter_mut()
                .find(|p| p.name == view.name && p.location == view.location)
            {
                Some(existing) => *existing = view,
                None => params.push(view),
            }
        }
    }
    Ok(params)
}

fn check_ambiguous_operation_ids(ops: &[OperationView]) -> Result<()> {
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for op in ops {
        if let Some(id) = &op.identity.operation_id {
            seen.entry(id.clone())
                .or_default()
                .push(format!("{} {}", op.identity.method, op.identity.path));
        }
    }
    for (id, uses) in seen {
        if uses.len() > 1 {
            return Err(Error::AmbiguousOperation {
                location: format!("operationId {id}"),
                message: format!("used by {}", uses.join(", ")),
            });
        }
    }
    Ok(())
}

fn extract_json_body(
    doc: &OpenApiDoc,
    container: &Value,
    location: &str,
    budget: &mut crate::schema::ResolveBudget,
) -> Result<BodyView> {
    let content = container
        .get("content")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Unsupported {
            construct: "media_type".into(),
            location: location.into(),
            message: "JSON body requires content.application/json".into(),
        })?;
    let json_keys: Vec<&String> = content
        .keys()
        .filter(|k| *k == "application/json" || k.starts_with("application/json;"))
        .collect();
    if json_keys.is_empty() {
        let found: Vec<_> = content.keys().cloned().collect();
        return Err(Error::Unsupported {
            construct: "media_type".into(),
            location: location.into(),
            message: format!("no application/json body; found {}", found.join(", ")),
        });
    }
    if json_keys.len() > 1 {
        return Err(Error::AmbiguousOperation {
            location: location.into(),
            message: "multiple application/json media types on one body".into(),
        });
    }
    let ct = json_keys[0].clone();
    let media = &content[&ct];
    let schema = media.get("schema").cloned().ok_or_else(|| Error::Parse {
        path: location.into(),
        message: "media type missing schema".into(),
    })?;
    let pointer = format!("{location}/content/{}/schema", pointer_escape(&ct));
    let mut resolved = crate::schema::resolve_schema_within(doc, &schema, &pointer, budget)?;
    if content.len() > 1 {
        let extras: Vec<_> = content.keys().filter(|k| *k != &ct).cloned().collect();
        resolved.unsupported.push(crate::schema::UnsupportedNote {
            construct: "media_type".into(),
            location: location.into(),
            message: format!(
                "non-JSON media types are not mapped and remain visible: {}",
                extras.join(", ")
            ),
        });
    }
    if resolved.properties_len() > MAX_PROPERTIES {
        return Err(Error::Bound {
            kind: "properties".into(),
            location: pointer.clone(),
            message: format!("more than {MAX_PROPERTIES} properties"),
        });
    }
    Ok(BodyView {
        content_type: ct,
        pointer,
        schema,
        resolved,
    })
}

pub fn duplicate_operation_ids(ops: &[OperationView]) -> BTreeSet<String> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for op in ops {
        if let Some(id) = &op.identity.operation_id {
            *counts.entry(id.clone()).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(k, n)| if n > 1 { Some(k) } else { None })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_line_index_maps_pointers_to_lines() {
        let text =
            "{\n  \"a/b\": {\n    \"c\": [1,\n      {\"d\": \"x\\\"y\"}]\n  },\n  \"e\": true\n}\n";
        let index = json_line_index(text);
        assert_eq!(index[""], 1);
        assert_eq!(index["/a~1b"], 2);
        assert_eq!(index["/a~1b/c"], 3);
        assert_eq!(index["/a~1b/c/0"], 3);
        assert_eq!(index["/a~1b/c/1"], 4);
        assert_eq!(index["/a~1b/c/1/d"], 4);
        assert_eq!(index["/e"], 6);
    }

    #[test]
    fn rejects_remote_and_traversal_refs() {
        assert!(validate_local_ref("https://example.com/x.yaml#/Foo", "$").is_err());
        assert!(validate_local_ref("#/components/../secrets", "$").is_err());
        assert!(validate_local_ref("#/components/schemas/User", "$").is_ok());
    }

    #[test]
    fn absolute_paths_are_reduced_to_file_names() {
        let absolute = std::env::current_dir()
            .unwrap()
            .join("private")
            .join("base.openapi.yaml");
        assert_eq!(portable_path(&absolute), "base.openapi.yaml");
        assert_eq!(
            portable_path(Path::new("packs\\rename-user-id\\base.openapi.yaml")),
            "packs/rename-user-id/base.openapi.yaml"
        );
    }
}
