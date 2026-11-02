use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::digest::MAX_DIFF_ENTRIES;
use crate::error::{Error, Result};
use crate::parse::{
    load_openapi, operations, pointer_escape, pointer_unescape, resolve_pointer, BodyView,
    OpIdentity, OpenApiDoc, OperationView,
};
use crate::schema::{AdditionalProperties, ResolvedSchema};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffDocument {
    pub schema_version: String,
    pub kind: String,
    pub source: ContractRef,
    pub destination: ContractRef,
    pub operations: Vec<OperationDiff>,
    pub added_operations: Vec<OpIdentity>,
    pub removed_operations: Vec<OpIdentity>,
    /// Operations whose method or path changed while their `operationId`
    /// stayed; each pair is also diffed in `operations`.
    #[serde(default)]
    pub moved_operations: Vec<MovedOperation>,
    /// Every component schema that differs between the contracts, with every
    /// operation (on either side) whose bodies reach it through `$ref`s.
    #[serde(default)]
    pub affected_schemas: Vec<AffectedSchema>,
    pub unsupported: Vec<crate::schema::UnsupportedNote>,
    pub unknown: Vec<crate::schema::UnsupportedNote>,
    #[serde(default)]
    pub imported_evidence: Vec<crate::oasdiff::ImportedEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MovedOperation {
    pub from: OpIdentity,
    pub to: OpIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AffectedSchema {
    /// RFC 6901 pointer of the component, e.g. `/components/schemas/Address`.
    pub component_pointer: String,
    pub operations: Vec<OpIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractRef {
    pub path: String,
    pub digest: String,
    pub openapi: String,
    pub dialect: crate::parse::Dialect,
    pub title: String,
    /// `json_key_path` when changes carry line numbers in this contract;
    /// `line_unavailable` for YAML, whose positions are not tracked.
    #[serde(default)]
    pub line_numbers: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationDiff {
    pub identity: OpIdentity,
    pub destination_identity: OpIdentity,
    pub request: Option<BodyDiff>,
    pub responses: BTreeMap<String, BodyDiff>,
    pub notes: Vec<String>,
    /// Added and removed path/query/header/cookie parameters and changes to
    /// `required`. Parameters are compared, not mapped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameter_changes: Vec<ParameterChange>,
    /// Worst class over the request, every response and every parameter change.
    #[serde(default)]
    pub compatibility: Compatibility,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParameterChange {
    pub name: String,
    /// `path`, `query`, `header` or `cookie`.
    #[serde(rename = "in")]
    pub location: String,
    /// `added`, `removed`, `required_added` or `required_removed`.
    pub kind: String,
    pub compatibility: Compatibility,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyDiff {
    pub location: String,
    /// Worst compatibility of the body: `breaking` if any change breaks,
    /// otherwise `unknown` if any change or any unsupported construct cannot
    /// be classified, otherwise `non_breaking`, or `unchanged`.
    pub compatibility: Compatibility,
    /// Every classified change at any depth reachable through `properties`
    /// and `items`, in document order.
    pub changes: Vec<Change>,
    pub added_properties: Vec<String>,
    pub removed_properties: Vec<String>,
    pub changed_properties: Vec<PropertyChange>,
    pub required_added: Vec<String>,
    pub required_removed: Vec<String>,
    /// Notes for constructs in this body. Not serialized here: every note is
    /// emitted once, in the document's `unsupported`/`unknown` lists, whose
    /// `location` names the body.
    #[serde(skip)]
    pub unsupported: Vec<crate::schema::UnsupportedNote>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    #[default]
    Unchanged,
    NonBreaking,
    Unknown,
    Breaking,
}

impl Compatibility {
    /// Every class, in severity order; the diff schema's enum must match.
    pub const ALL: [Compatibility; 4] = [
        Compatibility::Unchanged,
        Compatibility::NonBreaking,
        Compatibility::Unknown,
        Compatibility::Breaking,
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Change {
    /// RFC 6901 pointer to the schema in the source contract's body schema,
    /// following `properties/<name>` and `items` through local `$ref`s.
    pub pointer: String,
    /// Payload path of the field (`address.zip`, `lines[].price`); empty for
    /// the body itself.
    pub field: String,
    pub kind: ChangeKind,
    pub compatibility: Compatibility,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<Value>,
    /// Line in the source contract where the changed schema (or, for a
    /// `required` change, the `required` list) is written; `null` when the
    /// contract is YAML or the position does not exist on that side.
    #[serde(default)]
    pub source_line: Option<usize>,
    #[serde(default)]
    pub destination_line: Option<usize>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    PropertyAdded,
    PropertyRemoved,
    RequiredAdded,
    RequiredRemoved,
    TypeChanged,
    EnumChanged,
    AdditionalPropertiesChanged,
    KeywordChanged,
    AnnotationChanged,
    UnsupportedChanged,
}

impl ChangeKind {
    /// Every kind; the diff schema's enum must match.
    pub const ALL: [ChangeKind; 10] = [
        ChangeKind::PropertyAdded,
        ChangeKind::PropertyRemoved,
        ChangeKind::RequiredAdded,
        ChangeKind::RequiredRemoved,
        ChangeKind::TypeChanged,
        ChangeKind::EnumChanged,
        ChangeKind::AdditionalPropertiesChanged,
        ChangeKind::KeywordChanged,
        ChangeKind::AnnotationChanged,
        ChangeKind::UnsupportedChanged,
    ];
}

/// Who reads the body. A request is read by the provider, so accepting less
/// (narrowing) breaks callers; a response is read by the caller, so sending
/// more (widening) breaks callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Request,
    Response,
}

/// What a body comparison needs besides the two schemas.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    direction: Direction,
    source: &'a OpenApiDoc,
    dest: &'a OpenApiDoc,
}

impl Ctx<'_> {
    fn lines(&self, source: Option<&str>, dest: Option<&str>) -> (Option<usize>, Option<usize>) {
        (
            source.and_then(|p| self.source.line_of(p)),
            dest.and_then(|p| self.dest.line_of(p)),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyChange {
    pub name: String,
    pub source: Value,
    pub destination: Value,
    pub evidence: String,
}

pub fn compare_paths(source: &Path, destination: &Path) -> Result<DiffDocument> {
    let source_doc = load_openapi(source)?;
    let dest_doc = load_openapi(destination)?;
    compare_docs(&source_doc, &dest_doc)
}

pub fn compare_docs(source: &OpenApiDoc, dest: &OpenApiDoc) -> Result<DiffDocument> {
    let source_ops = operations(source)?;
    let dest_ops = operations(dest)?;
    let mut dest_by_key: BTreeMap<(String, String), &OperationView> = BTreeMap::new();
    for op in &dest_ops {
        dest_by_key.insert((op.identity.method.clone(), op.identity.path.clone()), op);
    }
    let key = |op: &OperationView| (op.identity.method.clone(), op.identity.path.clone());
    let source_keys: BTreeSet<_> = source_ops.iter().map(key).collect();
    // Leftovers on each side, paired by operationId (unique per contract).
    let unmatched_dest: BTreeMap<&str, &OperationView> = dest_ops
        .iter()
        .filter(|op| !source_keys.contains(&key(op)))
        .filter_map(|op| op.identity.operation_id.as_deref().map(|id| (id, op)))
        .collect();
    let mut operation_diffs = Vec::new();
    let mut moved = Vec::new();
    let mut moved_to = BTreeSet::new();
    let mut removed = Vec::new();
    let mut unsupported = Vec::new();
    let mut unknown = Vec::new();
    let mut seen_notes = BTreeSet::new();
    let mut changes = 0;
    for op in &source_ops {
        let partner = match dest_by_key.get(&key(op)) {
            Some(dest_op) => Some(*dest_op),
            None => {
                let paired = op
                    .identity
                    .operation_id
                    .as_deref()
                    .and_then(|id| unmatched_dest.get(id).copied());
                if let Some(dest_op) = paired {
                    moved.push(MovedOperation {
                        from: op.identity.clone(),
                        to: dest_op.identity.clone(),
                    });
                    moved_to.insert(key(dest_op));
                }
                paired
            }
        };
        match partner {
            Some(dest_op) => {
                let diff = diff_operation(op, dest_op, source, dest);
                collect_notes(&diff, &mut seen_notes, &mut unsupported, &mut unknown);
                changes += change_count(&diff);
                if changes + seen_notes.len() > MAX_DIFF_ENTRIES {
                    return Err(Error::Bound {
                        kind: "diff_entries".into(),
                        location: format!("{} {}", op.identity.method, op.identity.path),
                        message: format!(
                            "the diff lists more than {MAX_DIFF_ENTRIES} changes and notes"
                        ),
                    });
                }
                operation_diffs.push(diff);
            }
            None => removed.push(op.identity.clone()),
        }
    }
    let added = dest_ops
        .iter()
        .filter(|op| !source_keys.contains(&key(op)) && !moved_to.contains(&key(op)))
        .map(|op| op.identity.clone())
        .collect();
    Ok(DiffDocument {
        schema_version: "1".into(),
        kind: "diff".into(),
        source: contract_ref(source),
        destination: contract_ref(dest),
        operations: operation_diffs,
        added_operations: added,
        removed_operations: removed,
        moved_operations: moved,
        affected_schemas: affected_schemas(source, &source_ops, dest, &dest_ops),
        unsupported,
        unknown,
        imported_evidence: Vec::new(),
    })
}

/// Component schemas that differ between the contracts, each with the
/// operations whose request or response bodies reach it through `$ref`s.
fn affected_schemas(
    source: &OpenApiDoc,
    source_ops: &[OperationView],
    dest: &OpenApiDoc,
    dest_ops: &[OperationView],
) -> Vec<AffectedSchema> {
    let components = |doc: &OpenApiDoc| {
        doc.value
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    let (before, after) = (components(source), components(dest));
    let changed: BTreeSet<&String> = before
        .keys()
        .chain(after.keys())
        .filter(|name| before.get(*name) != after.get(*name))
        .collect();
    let mut users: BTreeMap<&String, BTreeMap<(String, String), OpIdentity>> = BTreeMap::new();
    for (doc, ops) in [(source, source_ops), (dest, dest_ops)] {
        for op in ops {
            let mut reached = BTreeSet::new();
            let bodies = op.request.iter().chain(op.responses.values());
            for body in bodies {
                collect_refs(doc, &body.schema, &mut reached);
            }
            for name in reached {
                if let Some(component) = changed.get(&name) {
                    users.entry(*component).or_default().insert(
                        (op.identity.method.clone(), op.identity.path.clone()),
                        op.identity.clone(),
                    );
                }
            }
        }
    }
    users
        .into_iter()
        .map(|(name, ops)| AffectedSchema {
            component_pointer: format!("/components/schemas/{}", pointer_escape(name)),
            operations: ops.into_values().collect(),
        })
        .collect()
}

/// Names of every component schema reachable from `schema` through local
/// `$ref`s, transitively; each component is walked once.
fn collect_refs(doc: &OpenApiDoc, schema: &Value, reached: &mut BTreeSet<String>) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get("$ref") {
                if let Some(rest) = reference.strip_prefix("#/components/schemas/") {
                    let name = pointer_unescape(rest.split('/').next().unwrap_or(rest));
                    if reached.insert(name) {
                        if let Ok(target) = resolve_pointer(&doc.value, reference) {
                            collect_refs(doc, target, reached);
                        }
                    }
                }
            }
            for value in map.values() {
                collect_refs(doc, value, reached);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_refs(doc, i, reached)),
        _ => {}
    }
}

fn contract_ref(doc: &OpenApiDoc) -> ContractRef {
    ContractRef {
        path: crate::parse::portable_path(&doc.path),
        digest: doc.digest.clone(),
        openapi: doc.openapi_version.clone(),
        dialect: doc.dialect,
        title: doc.title.clone(),
        line_numbers: if doc.line_index.is_some() {
            "json_key_path".into()
        } else {
            "line_unavailable".into()
        },
    }
}

fn diff_operation(
    source: &OperationView,
    dest: &OperationView,
    source_doc: &OpenApiDoc,
    dest_doc: &OpenApiDoc,
) -> OperationDiff {
    let ctx = |direction| Ctx {
        direction,
        source: source_doc,
        dest: dest_doc,
    };
    let mut notes = source.notes.clone();
    notes.extend(dest.notes.iter().cloned());
    if source.identity.method != dest.identity.method || source.identity.path != dest.identity.path
    {
        notes.push(format!(
            "operation moved from {} {} to {} {}",
            source.identity.method, source.identity.path, dest.identity.method, dest.identity.path
        ));
    }
    if source.identity.operation_id != dest.identity.operation_id {
        notes.push(format!(
            "operationId changed from {:?} to {:?}",
            source.identity.operation_id, dest.identity.operation_id
        ));
    }
    let request = match (&source.request, &dest.request) {
        (Some(s), Some(d)) => Some(diff_body(
            "request",
            &body_pointer(&source.identity, "request", &s.content_type),
            ctx(Direction::Request),
            s,
            d,
        )),
        (None, None) => None,
        (Some(s), None) => Some(presence_body(
            "request",
            &body_pointer(&source.identity, "request", &s.content_type),
            (source_doc.line_of(&s.pointer), None),
            Presence {
                construct: "request_presence",
                note_location: &source.pointer,
                note: "request body present on only one side",
                kind: ChangeKind::PropertyRemoved,
                compatibility: Compatibility::Breaking,
                reason:
                    "the request body was removed; callers that send it are no longer described",
            },
        )),
        (None, Some(d)) => Some(presence_body(
            "request",
            &body_pointer(&dest.identity, "request", &d.content_type),
            (None, dest_doc.line_of(&d.pointer)),
            Presence {
                construct: "request_presence",
                note_location: &source.pointer,
                note: "request body present on only one side",
                kind: ChangeKind::PropertyAdded,
                compatibility: Compatibility::Unknown,
                reason: "a request body was added; whether callers must send it is not classified",
            },
        )),
    };
    let mut responses = BTreeMap::new();
    let mut statuses: Vec<String> = source.responses.keys().cloned().collect();
    for status in dest.responses.keys() {
        if !statuses.contains(status) {
            statuses.push(status.clone());
        }
    }
    for status in statuses {
        let location = format!("response {status}");
        let place = format!("responses/{status}");
        match (source.responses.get(&status), dest.responses.get(&status)) {
            (Some(s), Some(d)) => {
                responses.insert(
                    status.clone(),
                    diff_body(
                        &location,
                        &body_pointer(&source.identity, &place, &s.content_type),
                        ctx(Direction::Response),
                        s,
                        d,
                    ),
                );
            }
            (Some(s), None) => {
                responses.insert(
                    status.clone(),
                    presence_body(
                        &location,
                        &body_pointer(&source.identity, &place, &s.content_type),
                        (source_doc.line_of(&s.pointer), None),
                        Presence {
                            construct: "response_presence",
                            note_location: &source.pointer,
                            note: "response present only on the source",
                            kind: ChangeKind::PropertyRemoved,
                            compatibility: Compatibility::Breaking,
                            reason: "the response body was removed; callers that read it lose it",
                        },
                    ),
                );
            }
            (None, Some(d)) => {
                responses.insert(
                    status.clone(),
                    presence_body(
                        &location,
                        &body_pointer(&dest.identity, &place, &d.content_type),
                        (None, dest_doc.line_of(&d.pointer)),
                        Presence {
                            construct: "response_presence",
                            note_location: &dest.pointer,
                            note: "response present only on the destination",
                            kind: ChangeKind::PropertyAdded,
                            compatibility: Compatibility::NonBreaking,
                            reason: "a response body was added",
                        },
                    ),
                );
            }
            (None, None) => {}
        }
    }
    let parameter_changes = parameter_changes(&source.parameters, &dest.parameters);
    let compatibility = request
        .iter()
        .chain(responses.values())
        .map(|b| b.compatibility)
        .chain(parameter_changes.iter().map(|p| p.compatibility))
        .max()
        .unwrap_or(Compatibility::Unchanged);
    OperationDiff {
        identity: source.identity.clone(),
        destination_identity: dest.identity.clone(),
        request,
        responses,
        notes,
        parameter_changes,
        compatibility,
    }
}

/// Classify parameter differences by name and location. A new required
/// parameter or an optional one becoming required breaks existing callers; a
/// removed parameter is `unknown` (whether the server ignores or rejects one
/// that callers still send is not described).
fn parameter_changes(
    source: &[crate::parse::ParamView],
    dest: &[crate::parse::ParamView],
) -> Vec<ParameterChange> {
    let mut out = Vec::new();
    let change =
        |p: &crate::parse::ParamView, kind: &str, compatibility, reason: &str| ParameterChange {
            name: p.name.clone(),
            location: p.location.clone(),
            kind: kind.into(),
            compatibility,
            reason: reason.into(),
        };
    for d in dest {
        match source
            .iter()
            .find(|s| s.name == d.name && s.location == d.location)
        {
            None if d.required => out.push(change(
                d,
                "added",
                Compatibility::Breaking,
                "a required parameter was added; existing callers do not send it",
            )),
            None => out.push(change(
                d,
                "added",
                Compatibility::NonBreaking,
                "an optional parameter was added",
            )),
            Some(s) if !s.required && d.required => out.push(change(
                d,
                "required_added",
                Compatibility::Breaking,
                "an optional parameter became required; callers that omit it break",
            )),
            Some(s) if s.required && !d.required => out.push(change(
                d,
                "required_removed",
                Compatibility::NonBreaking,
                "a required parameter became optional",
            )),
            Some(_) => {}
        }
    }
    for s in source {
        if !dest
            .iter()
            .any(|d| d.name == s.name && d.location == s.location)
        {
            out.push(change(s, "removed", Compatibility::Unknown, "the parameter was removed; whether the server ignores or rejects it when callers still send it is not described"));
        }
    }
    out
}

/// RFC 6901 pointer of a body schema in the contract: `place` is `request`
/// or `responses/<status>`.
fn body_pointer(identity: &OpIdentity, place: &str, content_type: &str) -> String {
    let container = match place.strip_prefix("responses/") {
        Some(status) => format!("responses/{}", pointer_escape(status)),
        None => "requestBody".to_string(),
    };
    format!(
        "/paths/{}/{}/{container}/content/{}/schema",
        pointer_escape(&identity.path),
        identity.method.to_ascii_lowercase(),
        pointer_escape(content_type)
    )
}

/// A body that exists on one side only.
struct Presence<'a> {
    construct: &'a str,
    note_location: &'a str,
    note: &'a str,
    kind: ChangeKind,
    compatibility: Compatibility,
    reason: &'a str,
}

fn presence_body(
    location: &str,
    pointer: &str,
    (source_line, destination_line): (Option<usize>, Option<usize>),
    presence: Presence<'_>,
) -> BodyDiff {
    BodyDiff {
        location: location.into(),
        compatibility: presence.compatibility,
        changes: vec![Change {
            pointer: pointer.into(),
            field: String::new(),
            kind: presence.kind,
            compatibility: presence.compatibility,
            reason: presence.reason.into(),
            source: None,
            destination: None,
            source_line,
            destination_line,
        }],
        added_properties: Vec::new(),
        removed_properties: Vec::new(),
        changed_properties: Vec::new(),
        required_added: Vec::new(),
        required_removed: Vec::new(),
        unsupported: vec![crate::schema::UnsupportedNote {
            construct: presence.construct.into(),
            location: presence.note_location.into(),
            message: presence.note.into(),
        }],
    }
}

fn diff_body(
    location: &str,
    pointer: &str,
    ctx: Ctx<'_>,
    source: &BodyView,
    dest: &BodyView,
) -> BodyDiff {
    let mut unsupported = Vec::new();
    let mut seen = BTreeSet::new();
    for note in source
        .resolved
        .all_unsupported()
        .into_iter()
        .chain(dest.resolved.all_unsupported())
    {
        if seen.insert(note.clone()) {
            unsupported.push(note);
        }
    }
    let mut changes = Vec::new();
    diff_node(
        ctx,
        &source.resolved,
        &dest.resolved,
        pointer,
        "",
        &mut changes,
    );
    let mut compatibility = changes
        .iter()
        .map(|c| c.compatibility)
        .max()
        .unwrap_or(Compatibility::Unchanged);
    if !unsupported.is_empty() && compatibility < Compatibility::Unknown {
        compatibility = Compatibility::Unknown;
    }
    // The per-name lists below compare top-level properties. When either
    // root is a construct the tool cannot read (a combinator, an unresolved
    // reference), that side has no property list to compare, so nothing is
    // derived from it: the body is `unknown` and `changes` says why.
    let unreadable = |body: &BodyView| {
        body.resolved
            .unsupported
            .iter()
            .any(|note| note.construct != "media_type")
    };
    if unreadable(source) || unreadable(dest) {
        return BodyDiff {
            location: location.into(),
            compatibility,
            changes,
            added_properties: Vec::new(),
            removed_properties: Vec::new(),
            changed_properties: Vec::new(),
            required_added: Vec::new(),
            required_removed: Vec::new(),
            unsupported,
        };
    }
    let src_props: Vec<_> = source.resolved.properties.keys().cloned().collect();
    let dst_props: Vec<_> = dest.resolved.properties.keys().cloned().collect();
    let added: Vec<_> = dst_props
        .iter()
        .filter(|p| !src_props.contains(p))
        .cloned()
        .collect();
    let removed: Vec<_> = src_props
        .iter()
        .filter(|p| !dst_props.contains(p))
        .cloned()
        .collect();
    let mut changed = Vec::new();
    for name in src_props.iter().filter(|p| dst_props.contains(p)) {
        let s = &source.resolved.properties[name];
        let d = &dest.resolved.properties[name];
        if property_fingerprint(s) != property_fingerprint(d) {
            changed.push(PropertyChange {
                name: name.clone(),
                source: s.original.clone(),
                destination: d.original.clone(),
                evidence: format!("{} vs {}", s.location, d.location),
            });
        }
    }
    let required_added: Vec<_> = dest
        .resolved
        .required
        .iter()
        .filter(|r| !source.resolved.required.contains(r))
        .cloned()
        .collect();
    let required_removed: Vec<_> = source
        .resolved
        .required
        .iter()
        .filter(|r| !dest.resolved.required.contains(r))
        .cloned()
        .collect();
    BodyDiff {
        location: location.into(),
        compatibility,
        changes,
        added_properties: added,
        removed_properties: removed,
        changed_properties: changed,
        required_added,
        required_removed,
        unsupported,
    }
}

/// Keywords compared semantically by `diff_node`.
const MODELED_KEYWORDS: &[&str] = &[
    "type",
    "nullable",
    "enum",
    "properties",
    "required",
    "additionalProperties",
    "items",
];

/// Keywords that document a schema without constraining instances.
const ANNOTATION_KEYWORDS: &[&str] = &[
    "description",
    "title",
    "example",
    "examples",
    "externalDocs",
    "xml",
    "deprecated",
    "$comment",
];

fn is_annotation(keyword: &str) -> bool {
    ANNOTATION_KEYWORDS.contains(&keyword) || keyword.starts_with("x-")
}

/// Compare one schema node and recurse into `properties` and `items`.
/// Every keyword is either compared semantically, reported as an annotation
/// change, or reported as an unclassified (`unknown`) keyword change, so a
/// changed schema can never produce an empty change list.
fn diff_node(
    ctx: Ctx<'_>,
    s: &ResolvedSchema,
    d: &ResolvedSchema,
    pointer: &str,
    field: &str,
    out: &mut Vec<Change>,
) {
    let direction = ctx.direction;
    let (source_line, destination_line) = ctx.lines(Some(&s.physical), Some(&d.physical));
    let change = |kind, compatibility, reason: String, source, destination| Change {
        pointer: pointer.to_string(),
        field: field.to_string(),
        kind,
        compatibility,
        reason,
        source,
        destination,
        source_line,
        destination_line,
    };
    if !s.unsupported.is_empty() || !d.unsupported.is_empty() {
        if s.original != d.original {
            let mut constructs: Vec<String> = s
                .unsupported
                .iter()
                .chain(d.unsupported.iter())
                .map(|n| n.construct.clone())
                .collect();
            constructs.sort();
            constructs.dedup();
            out.push(change(
                ChangeKind::UnsupportedChanged,
                Compatibility::Unknown,
                format!(
                    "schema changed and uses constructs outside the supported subset ({}); not classified",
                    constructs.join(", ")
                ),
                Some(s.original.clone()),
                Some(d.original.clone()),
            ));
        }
        return;
    }

    let (type_widened, type_narrowed) = type_delta(&s.types, &d.types);
    if type_widened || type_narrowed {
        out.push(change(
            ChangeKind::TypeChanged,
            set_change(direction, type_widened, type_narrowed),
            format!("type {} -> {}", type_label(&s.types), type_label(&d.types)),
            Some(json!(s.types)),
            Some(json!(d.types)),
        ));
    }

    let (enum_widened, enum_narrowed) = enum_delta(&s.enum_values, &d.enum_values);
    if enum_widened || enum_narrowed {
        out.push(change(
            ChangeKind::EnumChanged,
            set_change(direction, enum_widened, enum_narrowed),
            match (enum_widened, enum_narrowed) {
                (true, true) => "enum values added and removed".into(),
                (true, false) => "enum grown or removed".into(),
                _ => "enum shrunk or introduced".into(),
            },
            s.enum_values.as_ref().map(|v| json!(v)),
            d.enum_values.as_ref().map(|v| json!(v)),
        ));
    }

    if s.additional_properties != d.additional_properties {
        let widened = d.additional_properties == AdditionalProperties::Allowed;
        out.push(change(
            ChangeKind::AdditionalPropertiesChanged,
            set_change(direction, widened, !widened),
            if widened {
                "additionalProperties forbidden -> allowed".into()
            } else {
                "additionalProperties allowed -> forbidden".into()
            },
            None,
            None,
        ));
    }

    let mut unclassified = Vec::new();
    let mut annotations = Vec::new();
    for keyword in keyword_union(&s.original, &d.original) {
        if MODELED_KEYWORDS.contains(&keyword.as_str()) {
            continue;
        }
        if s.original.get(&keyword) != d.original.get(&keyword) {
            if is_annotation(&keyword) {
                annotations.push(keyword);
            } else {
                unclassified.push(keyword);
            }
        }
    }
    if !unclassified.is_empty() {
        out.push(change(
            ChangeKind::KeywordChanged,
            Compatibility::Unknown,
            format!(
                "keyword(s) {} changed; this tool does not classify them",
                unclassified.join(", ")
            ),
            Some(pick(&s.original, &unclassified)),
            Some(pick(&d.original, &unclassified)),
        ));
    }
    if !annotations.is_empty() {
        out.push(change(
            ChangeKind::AnnotationChanged,
            Compatibility::NonBreaking,
            format!("annotation(s) {} changed", annotations.join(", ")),
            Some(pick(&s.original, &annotations)),
            Some(pick(&d.original, &annotations)),
        ));
    }

    if allows(&s.types, "object") && allows(&d.types, "object") {
        diff_properties(ctx, s, d, pointer, field, out);
    }
    if allows(&s.types, "array") && allows(&d.types, "array") {
        if let (Some(si), Some(di)) = (&s.items, &d.items) {
            diff_node(
                ctx,
                si,
                di,
                &format!("{pointer}/items"),
                &format!("{field}[]"),
                out,
            );
        }
    }
}

fn diff_properties(
    ctx: Ctx<'_>,
    s: &ResolvedSchema,
    d: &ResolvedSchema,
    pointer: &str,
    field: &str,
    out: &mut Vec<Change>,
) {
    let direction = ctx.direction;
    // A `required` change is located at the parent's `required` list.
    let required_lines = ctx.lines(
        Some(&format!("{}/required", s.physical)),
        Some(&format!("{}/required", d.physical)),
    );
    let at = |name: &str| {
        (
            format!("{pointer}/properties/{}", pointer_escape(name)),
            join_field(field, name),
        )
    };
    for (name, child) in &s.properties {
        let (child_pointer, child_field) = at(name);
        match d.properties.get(name) {
            Some(other) => {
                diff_node(ctx, child, other, &child_pointer, &child_field, out);
                let was = s.required.contains(name);
                let is = d.required.contains(name);
                if was != is {
                    out.push(required_change(
                        direction,
                        is,
                        (child_pointer, child_field),
                        required_lines,
                        if is {
                            format!("{name} became required")
                        } else {
                            format!("{name} is no longer required")
                        },
                    ));
                }
            }
            None => out.push(Change {
                pointer: child_pointer,
                field: child_field,
                kind: ChangeKind::PropertyRemoved,
                compatibility: Compatibility::Breaking,
                reason: format!("property {name} was removed"),
                source: Some(child.original.clone()),
                destination: None,
                source_line: ctx.source.line_of(&child.physical),
                destination_line: None,
            }),
        }
    }
    for (name, child) in &d.properties {
        if s.properties.contains_key(name) {
            continue;
        }
        let (child_pointer, child_field) = at(name);
        let required = d.required.contains(name);
        out.push(Change {
            pointer: child_pointer,
            field: child_field,
            kind: ChangeKind::PropertyAdded,
            compatibility: if required && direction == Direction::Request {
                Compatibility::Breaking
            } else {
                Compatibility::NonBreaking
            },
            reason: if required {
                format!("required property {name} was added")
            } else {
                format!("optional property {name} was added")
            },
            source: None,
            destination: Some(child.original.clone()),
            source_line: None,
            destination_line: ctx.dest.line_of(&child.physical),
        });
    }
    // `required` names that no side declares as a property.
    let undeclared =
        |name: &String| !s.properties.contains_key(name) && !d.properties.contains_key(name);
    for name in d
        .required
        .iter()
        .filter(|r| !s.required.contains(r) && undeclared(r))
    {
        let (child_pointer, child_field) = at(name);
        out.push(required_change(
            direction,
            true,
            (child_pointer, child_field),
            required_lines,
            format!("undeclared property {name} became required"),
        ));
    }
    for name in s
        .required
        .iter()
        .filter(|r| !d.required.contains(r) && undeclared(r))
    {
        let (child_pointer, child_field) = at(name);
        out.push(required_change(
            direction,
            false,
            (child_pointer, child_field),
            required_lines,
            format!("undeclared property {name} is no longer required"),
        ));
    }
}

/// Requiring a field narrows what a request may omit and widens what a
/// response guarantees; dropping the requirement does the opposite.
fn required_change(
    direction: Direction,
    now_required: bool,
    (pointer, field): (String, String),
    (source_line, destination_line): (Option<usize>, Option<usize>),
    reason: String,
) -> Change {
    let breaking = matches!(
        (now_required, direction),
        (true, Direction::Request) | (false, Direction::Response)
    );
    Change {
        pointer,
        field,
        kind: if now_required {
            ChangeKind::RequiredAdded
        } else {
            ChangeKind::RequiredRemoved
        },
        compatibility: if breaking {
            Compatibility::Breaking
        } else {
            Compatibility::NonBreaking
        },
        reason,
        source: None,
        destination: None,
        source_line,
        destination_line,
    }
}

fn has(types: &[String], wanted: &str) -> bool {
    types.iter().any(|t| t == wanted)
}

/// An untyped schema (no `type`, an empty list) allows every type.
fn allows(types: &[String], wanted: &str) -> bool {
    types.is_empty() || has(types, wanted)
}

fn type_label(types: &[String]) -> String {
    if types.is_empty() {
        "any".into()
    } else {
        types.join("|")
    }
}

fn join_field(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}

/// Classify a change of an accepted-value set. Narrowing breaks writers of
/// requests; widening breaks readers of responses; both at once breaks.
fn set_change(direction: Direction, widened: bool, narrowed: bool) -> Compatibility {
    match (widened, narrowed, direction) {
        (false, false, _) => Compatibility::Unchanged,
        (true, true, _) => Compatibility::Breaking,
        (false, true, Direction::Request) | (true, false, Direction::Response) => {
            Compatibility::Breaking
        }
        (false, true, Direction::Response) | (true, false, Direction::Request) => {
            Compatibility::NonBreaking
        }
    }
}

/// `integer` values are also `number` values.
fn covers(types: &[String], t: &str) -> bool {
    has(types, t) || (t == "integer" && has(types, "number"))
}

fn type_delta(s: &[String], d: &[String]) -> (bool, bool) {
    // An empty list is "any type": typing it narrows, untyping a typed schema widens.
    match (s.is_empty(), d.is_empty()) {
        (true, true) => return (false, false),
        (true, false) => return (false, true),
        (false, true) => return (true, false),
        (false, false) => {}
    }
    let widened = d.iter().any(|t| !covers(s, t));
    let narrowed = s.iter().any(|t| !covers(d, t));
    (widened, narrowed)
}

fn enum_delta(s: &Option<Vec<Value>>, d: &Option<Vec<Value>>) -> (bool, bool) {
    match (s, d) {
        (None, None) => (false, false),
        (Some(_), None) => (true, false),
        (None, Some(_)) => (false, true),
        (Some(s), Some(d)) => (
            d.iter().any(|v| !s.contains(v)),
            s.iter().any(|v| !d.contains(v)),
        ),
    }
}

fn keyword_union(s: &Value, d: &Value) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for value in [s, d] {
        if let Some(map) = value.as_object() {
            for key in map.keys() {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
        }
    }
    keys.sort();
    keys
}

fn pick(value: &Value, keys: &[String]) -> Value {
    let mut out = serde_json::Map::new();
    for key in keys {
        if let Some(v) = value.get(key) {
            out.insert(key.clone(), v.clone());
        }
    }
    Value::Object(out)
}

fn property_fingerprint(schema: &ResolvedSchema) -> String {
    format!(
        "{:?}|{:?}|{:?}|{}",
        schema.types, schema.enum_values, schema.nullable, schema.dialect as u8
    )
}

/// Changes the operation lists: every body change and every parameter change.
fn change_count(diff: &OperationDiff) -> usize {
    diff.request
        .iter()
        .chain(diff.responses.values())
        .map(|body| body.changes.len())
        .sum::<usize>()
        + diff.parameter_changes.len()
}

fn collect_notes(
    diff: &OperationDiff,
    seen: &mut BTreeSet<crate::schema::UnsupportedNote>,
    unsupported: &mut Vec<crate::schema::UnsupportedNote>,
    unknown: &mut Vec<crate::schema::UnsupportedNote>,
) {
    let mut bodies: Vec<&BodyDiff> = Vec::new();
    if let Some(req) = &diff.request {
        bodies.push(req);
    }
    for body in diff.responses.values() {
        bodies.push(body);
    }
    for body in bodies {
        for note in &body.unsupported {
            let list = if note.construct == "unknown" {
                &mut *unknown
            } else {
                &mut *unsupported
            };
            if seen.insert(note.clone()) {
                list.push(note.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new variant must be added to `ALL` (and so to the schema): this
    /// match stops compiling when a variant is missing from it.
    #[test]
    fn all_lists_cover_every_variant() {
        for kind in ChangeKind::ALL {
            match kind {
                ChangeKind::PropertyAdded
                | ChangeKind::PropertyRemoved
                | ChangeKind::RequiredAdded
                | ChangeKind::RequiredRemoved
                | ChangeKind::TypeChanged
                | ChangeKind::EnumChanged
                | ChangeKind::AdditionalPropertiesChanged
                | ChangeKind::KeywordChanged
                | ChangeKind::AnnotationChanged
                | ChangeKind::UnsupportedChanged => {}
            }
        }
        let mut sorted = Compatibility::ALL;
        sorted.sort();
        assert_eq!(sorted, Compatibility::ALL);
    }
}
