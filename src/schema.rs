use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::digest::{
    MAX_ARRAY, MAX_ENUM, MAX_PATTERNS, MAX_PATTERN_BYTES, MAX_SCHEMA_BYTES, MAX_SCHEMA_NODES,
};
use crate::error::{Error, Result};
use crate::parse::{pointer_escape, resolve_ref, Dialect, OpenApiDoc};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedSchema {
    /// Logical RFC 6901 pointer: the body schema's pointer followed by
    /// `properties/<name>` and `items`, also below a `$ref`.
    pub location: String,
    /// Where this schema is written in the contract: the logical pointer, or
    /// the target of the `$ref` it came through. Used to locate lines.
    #[serde(default, skip_serializing)]
    pub physical: String,
    pub dialect: Dialect,
    pub nullable: bool,
    pub types: Vec<String>,
    pub enum_values: Option<Vec<Value>>,
    pub required: Vec<String>,
    pub properties: BTreeMap<String, ResolvedSchema>,
    pub additional_properties: AdditionalProperties,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<ResolvedSchema>>,
    pub unsupported: Vec<UnsupportedNote>,
    /// Bounds, `pattern` and `format` the schema declares; enforced by
    /// `validate_instance`.
    #[serde(default)]
    pub constraints: Constraints,
    pub original: Value,
}

/// The value constraints this tool enforces. OpenAPI 3.0's boolean
/// `exclusiveMinimum`/`exclusiveMaximum` beside `minimum`/`maximum` and
/// 3.1's numeric form both resolve to the exclusive bounds here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Constraints {
    /// Kept as written (an integer stays an integer) and compared exactly.
    pub minimum: Option<Number>,
    pub maximum: Option<Number>,
    pub exclusive_minimum: Option<Number>,
    pub exclusive_maximum: Option<Number>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub pattern: Option<String>,
    /// `pattern`, compiled once when the schema is resolved.
    #[serde(skip)]
    pub compiled_pattern: Option<CompiledPattern>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub unique_items: bool,
    pub format: Option<String>,
}

/// A compiled `pattern`, shared by every schema node that declares it.
#[derive(Debug, Clone)]
pub struct CompiledPattern(pub std::sync::Arc<regex::Regex>);

impl PartialEq for CompiledPattern {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

/// JSON Schema patterns are ECMA-262 regular expressions, where `\d`, `\w` and
/// `\b` are ASCII; the Rust engine makes them Unicode. Rewrite them (and `\D`,
/// `\W`, `\B`) as their ASCII forms, outside and inside character classes;
/// every other escape is kept as written.
fn ecma_classes(pattern: &str) -> String {
    const WORD: &str = "0-9A-Za-z_";
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut chars = pattern.chars();
    let mut in_class = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('d') if in_class > 0 => out.push_str("0-9"),
                Some('d') => out.push_str("[0-9]"),
                Some('D') => out.push_str("[^0-9]"),
                Some('w') if in_class > 0 => out.push_str(WORD),
                Some('w') => out.push_str(&format!("[{WORD}]")),
                Some('W') => out.push_str(&format!("[^{WORD}]")),
                Some('b') if in_class == 0 => out.push_str("(?-u:\\b)"),
                Some('B') if in_class == 0 => out.push_str("(?-u:\\B)"),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            '[' => {
                in_class += 1;
                out.push(c);
            }
            ']' if in_class > 0 => {
                in_class -= 1;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Equality as JSON Schema defines it for `enum`, `const` and `uniqueItems`:
/// numbers by value (`1` equals `1.0`), arrays item by item, objects by key.
pub fn json_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            compare_numbers(x, y) == Some(std::cmp::Ordering::Equal)
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_equal(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_equal(v, w)))
        }
        _ => a == b,
    }
}

/// A JSON Schema `integer`: a number with no fractional part (`1.0` too).
fn is_integer_value(value: &Value) -> bool {
    value.as_i64().is_some()
        || value.as_u64().is_some()
        || value
            .as_f64()
            .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

/// Compile a `pattern` within `MAX_PATTERN_BYTES` (compiled program and lazy
/// DFA cache): a larger one is an error, so it is reported as unsupported
/// instead of being compiled at full size.
pub fn compile_pattern(pattern: &str) -> std::result::Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(&ecma_classes(pattern))
        .size_limit(MAX_PATTERN_BYTES)
        .dfa_size_limit(MAX_PATTERN_BYTES)
        .build()
}

/// Formats `validate_instance` checks. Any other `format` is an annotation,
/// as JSON Schema defines it, and is not enforced.
pub const VALIDATED_FORMATS: &[&str] = &["date", "date-time", "uuid", "email"];

fn read_constraints(
    obj: &Map<String, Value>,
    location: &str,
    unsupported: &mut Vec<UnsupportedNote>,
    budget: &mut ResolveBudget,
) -> Result<Constraints> {
    let compiled = match obj.get("pattern") {
        Some(Value::String(p)) => Some(budget.pattern(p, location)?),
        _ => None,
    };
    let mut note = |construct: &str, message: String| {
        unsupported.push(UnsupportedNote {
            construct: construct.into(),
            location: location.into(),
            message,
        })
    };
    let number = |key: &str, note: &mut dyn FnMut(&str, String)| match obj.get(key) {
        None => None,
        Some(Value::Number(n)) => Some(n.clone()),
        Some(other) => {
            note(key, format!("{key} must be a number, got {other}"));
            None
        }
    };
    let mut c = Constraints {
        minimum: number("minimum", &mut note),
        maximum: number("maximum", &mut note),
        ..Constraints::default()
    };
    for (key, bound, plain) in [
        ("exclusiveMinimum", &mut c.exclusive_minimum, &mut c.minimum),
        ("exclusiveMaximum", &mut c.exclusive_maximum, &mut c.maximum),
    ] {
        match obj.get(key) {
            None | Some(Value::Bool(false)) => {}
            Some(Value::Bool(true)) => *bound = plain.take(),
            Some(Value::Number(n)) => *bound = Some(n.clone()),
            Some(other) => note(
                key,
                format!("{key} must be a boolean or a number, got {other}"),
            ),
        }
    }
    let count = |key: &str, note: &mut dyn FnMut(&str, String)| match obj.get(key) {
        None => None,
        Some(Value::Number(n)) if n.as_u64().is_some() => n.as_u64(),
        Some(other) => {
            note(
                key,
                format!("{key} must be a non-negative integer, got {other}"),
            );
            None
        }
    };
    c.min_length = count("minLength", &mut note);
    c.max_length = count("maxLength", &mut note);
    c.min_items = count("minItems", &mut note);
    c.max_items = count("maxItems", &mut note);
    c.unique_items = obj.get("uniqueItems") == Some(&Value::Bool(true));
    match (obj.get("pattern"), compiled) {
        (None, _) => {}
        (Some(Value::String(p)), Some(Ok(re))) => {
            c.pattern = Some(p.clone());
            c.compiled_pattern = Some(re);
        }
        (Some(Value::String(p)), Some(Err(e))) => note(
            "pattern",
            format!("pattern {p:?} uses syntax this tool's regular-expression engine does not support, or compiles beyond {MAX_PATTERN_BYTES} bytes ({e}); it is not evaluated"),
        ),
        (Some(other), _) => note("pattern", format!("pattern must be a string, got {other}")),
    }
    if let Some(Value::String(f)) = obj.get("format") {
        c.format = Some(f.clone());
    }
    Ok(c)
}

fn leap(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn digits(text: &str) -> Option<u32> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok())?
}

fn valid_date(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    let [y, m, d] = parts.as_slice() else {
        return false;
    };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return false;
    }
    let (Some(y), Some(m), Some(d)) = (digits(y), digits(m), digits(d)) else {
        return false;
    };
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap(y) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

/// RFC 3339 `date-time`: full-date, `T`, hh:mm:ss with optional fraction,
/// and a required `Z` or ±hh:mm offset.
fn valid_date_time(text: &str) -> bool {
    let Some((date, time)) = text.split_once(['T', 't']) else {
        return false;
    };
    if !valid_date(date) {
        return false;
    }
    let (clock, offset) = if let Some(rest) = time.strip_suffix(['Z', 'z']) {
        (rest, None)
    } else if let Some(i) = time.rfind(['+', '-']) {
        (&time[..i], Some(&time[i + 1..]))
    } else {
        return false;
    };
    let (clock, fraction) = clock.split_once('.').unwrap_or((clock, "0"));
    if digits(fraction).is_none() {
        return false;
    }
    let hms: Vec<&str> = clock.split(':').collect();
    let [h, m, s] = hms.as_slice() else {
        return false;
    };
    let two = |t: &str| (t.len() == 2).then(|| digits(t)).flatten();
    let (Some(h), Some(m), Some(s)) = (two(h), two(m), two(s)) else {
        return false;
    };
    if h > 23 || m > 59 || s > 60 {
        return false;
    }
    // Offset east of UTC in minutes: local time = UTC + offset.
    let offset_minutes = match offset {
        None => 0,
        Some(off) => {
            let Some((oh, om)) = off.split_once(':') else {
                return false;
            };
            let (Some(oh), Some(om)) = (two(oh), two(om)) else {
                return false;
            };
            if oh > 23 || om > 59 {
                return false;
            }
            let east = time.as_bytes()[time.len() - off.len() - 1] == b'+';
            let minutes = i64::from(oh * 60 + om);
            if east {
                minutes
            } else {
                -minutes
            }
        }
    };
    // RFC 3339 places a leap second at 23:59:60 UTC only. Which dates had one
    // is not checked (there is no leap-second table here).
    if s == 60 {
        let utc = (i64::from(h * 60 + m) - offset_minutes).rem_euclid(24 * 60);
        return utc == 23 * 60 + 59;
    }
    true
}

fn valid_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A pragmatic address check: one `@`, a non-empty local part without
/// spaces, no control or invisible (format) characters, and a dotted domain
/// whose labels are non-empty.
pub fn valid_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !text.contains(char::is_whitespace)
        && !text.chars().any(crate::report::is_hidden)
        && !domain.contains('@')
        && domain.contains('.')
        && domain.split('.').all(|label| !label.is_empty())
}

/// `Some(valid)` for a validated format, `None` for an annotation-only one.
pub fn check_format(format: &str, text: &str) -> Option<bool> {
    Some(match format {
        "date" => valid_date(text),
        "date-time" => valid_date_time(text),
        "uuid" => valid_uuid(text),
        "email" => valid_email(text),
        _ => return None,
    })
}

/// Order two JSON numbers exactly. Integers (`i64` or `u64`) compare as
/// integers; an integer against a fraction compares against the fraction's
/// floor, which is exact for every finite `f64`; two fractions compare as the
/// doubles they were read as. `None` only for a non-finite value.
pub fn compare_numbers(a: &Number, b: &Number) -> Option<std::cmp::Ordering> {
    fn int(n: &Number) -> Option<i128> {
        n.as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
    }
    fn int_vs_float(i: i128, f: f64) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering::{Equal, Greater, Less};
        if !f.is_finite() {
            return None;
        }
        // Every finite double of magnitude 2^127 or more is beyond any i128.
        const LIMIT: f64 = 170_141_183_460_469_231_731_687_303_715_884_105_728.0;
        if f >= LIMIT {
            return Some(Less);
        }
        if f < -LIMIT {
            return Some(Greater);
        }
        let floor = f.floor();
        Some(match i.cmp(&(floor as i128)) {
            Equal if f != floor => Less,
            order => order,
        })
    }
    match (int(a), int(b)) {
        (Some(x), Some(y)) => Some(x.cmp(&y)),
        (Some(x), None) => int_vs_float(x, b.as_f64()?),
        (None, Some(y)) => int_vs_float(y, a.as_f64()?).map(std::cmp::Ordering::reverse),
        (None, None) => a.as_f64()?.partial_cmp(&b.as_f64()?),
    }
}

fn check_constraints(schema: &ResolvedSchema, instance: &Value, location: &str) -> Result<()> {
    let c = &schema.constraints;
    let fail = |message: String| {
        Err(Error::SchemaValidation {
            location: location.into(),
            message,
        })
    };
    if let Value::Number(v) = instance {
        use std::cmp::Ordering::{Equal, Greater, Less};
        let checks = [
            (&c.minimum, [Less, Less], "is below minimum"),
            (&c.maximum, [Greater, Greater], "is above maximum"),
            (
                &c.exclusive_minimum,
                [Less, Equal],
                "is not above exclusive minimum",
            ),
            (
                &c.exclusive_maximum,
                [Greater, Equal],
                "is not below exclusive maximum",
            ),
        ];
        for (bound, refused, why) in checks {
            if let Some(m) = bound {
                if compare_numbers(v, m).is_none_or(|order| refused.contains(&order)) {
                    return fail(format!("{v} {why} {m}"));
                }
            }
        }
    }
    if let Some(text) = instance.as_str() {
        let len = text.chars().count() as u64;
        if c.min_length.is_some_and(|m| len < m) {
            return fail(format!(
                "length {len} is below minLength {}",
                c.min_length.unwrap()
            ));
        }
        if c.max_length.is_some_and(|m| len > m) {
            return fail(format!(
                "length {len} is above maxLength {}",
                c.max_length.unwrap()
            ));
        }
        if let Some(pattern) = &c.pattern {
            // Compiled once at resolve time; a hand-built schema compiles here.
            let compiled;
            let re = match &c.compiled_pattern {
                Some(CompiledPattern(re)) => re.as_ref(),
                None => {
                    compiled = compile_pattern(pattern).map_err(|e| Error::Unsupported {
                        construct: "pattern".into(),
                        location: location.into(),
                        message: e.to_string(),
                    })?;
                    &compiled
                }
            };
            if !re.is_match(text) {
                return fail(format!("value does not match pattern {pattern:?}"));
            }
        }
        if let Some(format) = &c.format {
            if check_format(format, text) == Some(false) {
                return fail(format!("value is not a valid {format}"));
            }
        }
    }
    if let Value::Array(items) = instance {
        let n = items.len() as u64;
        if c.min_items.is_some_and(|m| n < m) {
            return fail(format!(
                "{n} items is below minItems {}",
                c.min_items.unwrap()
            ));
        }
        if c.max_items.is_some_and(|m| n > m) {
            return fail(format!(
                "{n} items is above maxItems {}",
                c.max_items.unwrap()
            ));
        }
        if c.unique_items {
            for (i, item) in items.iter().enumerate() {
                if items[..i].iter().any(|earlier| json_equal(earlier, item)) {
                    return fail(format!(
                        "item {i} repeats an earlier item; uniqueItems is set"
                    ));
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdditionalProperties {
    Allowed,
    Forbidden,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct UnsupportedNote {
    pub construct: String,
    pub location: String,
    pub message: String,
}

impl ResolvedSchema {
    pub fn properties_len(&self) -> usize {
        self.properties.len()
    }

    pub fn is_unsupported(&self) -> bool {
        !self.unsupported.is_empty()
            || self.properties.values().any(ResolvedSchema::is_unsupported)
            || self
                .items
                .as_ref()
                .map(|items| items.is_unsupported())
                .unwrap_or(false)
    }

    pub fn all_unsupported(&self) -> Vec<UnsupportedNote> {
        let mut notes = self.unsupported.clone();
        for child in self.properties.values() {
            notes.extend(child.all_unsupported());
        }
        if let Some(items) = &self.items {
            notes.extend(items.all_unsupported());
        }
        notes
    }
}

/// What expanding `$ref`s in place may cost for one contract. Every resolved
/// node, and the bytes it copies, counts against `MAX_SCHEMA_NODES` and
/// `MAX_SCHEMA_BYTES`, so a small contract whose references fan out fails
/// closed (exit 3) instead of exhausting memory or time.
#[derive(Debug, Default)]
pub struct ResolveBudget {
    nodes: usize,
    bytes: usize,
    /// Each distinct `pattern` compiled once (or its compile error), at most
    /// `MAX_PATTERNS` of them.
    patterns: BTreeMap<String, std::result::Result<CompiledPattern, String>>,
}

impl ResolveBudget {
    fn pattern(
        &mut self,
        pattern: &str,
        location: &str,
    ) -> Result<std::result::Result<CompiledPattern, String>> {
        if let Some(done) = self.patterns.get(pattern) {
            return Ok(done.clone());
        }
        if self.patterns.len() >= MAX_PATTERNS {
            return Err(Error::Bound {
                kind: "patterns".into(),
                location: location.into(),
                message: format!(
                    "the contract declares more than {MAX_PATTERNS} distinct patterns"
                ),
            });
        }
        let compiled = compile_pattern(pattern)
            .map(|re| CompiledPattern(std::sync::Arc::new(re)))
            .map_err(|e| e.to_string());
        self.patterns.insert(pattern.to_string(), compiled.clone());
        Ok(compiled)
    }

    fn charge(&mut self, location: &str, nodes: usize, copied: &Value, extra: usize) -> Result<()> {
        self.nodes += nodes;
        if self.nodes > MAX_SCHEMA_NODES {
            return Err(Error::Bound {
                kind: "schema_nodes".into(),
                location: location.into(),
                message: format!(
                    "expanding $ref schemas in place needs more than {MAX_SCHEMA_NODES} schema nodes; the contract's references fan out too far"
                ),
            });
        }
        let room = MAX_SCHEMA_BYTES.saturating_sub(self.bytes);
        self.bytes += extra + value_size(copied, room);
        if self.bytes > MAX_SCHEMA_BYTES {
            return Err(Error::Bound {
                kind: "schema_bytes".into(),
                location: location.into(),
                message: format!(
                    "expanding $ref schemas in place copies more than {MAX_SCHEMA_BYTES} bytes; the contract's references fan out too far"
                ),
            });
        }
        Ok(())
    }
}

/// Approximate bytes a copy of `value` holds; stops counting past `limit`.
fn value_size(value: &Value, limit: usize) -> usize {
    let mut size = 16;
    match value {
        Value::String(text) => size += text.len(),
        Value::Array(items) => {
            for item in items {
                if size > limit {
                    break;
                }
                size += value_size(item, limit - size);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if size > limit {
                    break;
                }
                size += key.len() + value_size(item, limit - size);
            }
        }
        _ => {}
    }
    size
}

pub fn resolve_schema(doc: &OpenApiDoc, schema: &Value, location: &str) -> Result<ResolvedSchema> {
    resolve_schema_within(doc, schema, location, &mut ResolveBudget::default())
}

/// `resolve_schema`, charging the expansion to a budget shared by every body
/// of one contract.
pub fn resolve_schema_within(
    doc: &OpenApiDoc,
    schema: &Value,
    location: &str,
    budget: &mut ResolveBudget,
) -> Result<ResolvedSchema> {
    resolve_schema_inner(
        doc,
        schema,
        (location, location),
        &mut Vec::new(),
        &mut Vec::new(),
        budget,
    )
}

fn resolve_schema_inner(
    doc: &OpenApiDoc,
    schema: &Value,
    (location, physical): (&str, &str),
    ref_stack: &mut Vec<String>,
    schema_stack: &mut Vec<String>,
    budget: &mut ResolveBudget,
) -> Result<ResolvedSchema> {
    budget.charge(location, 1, schema, location.len() + physical.len())?;
    let mut unsupported = Vec::new();
    let mut working = schema.clone();
    let mut identity = location.to_string();
    let mut here = physical.to_string();
    if let Some(Value::String(r)) = schema.get("$ref") {
        identity = r.clone();
        here = r.trim_start_matches('#').to_string();
        if schema.as_object().map(|m| m.len() > 1).unwrap_or(false) {
            unsupported.push(UnsupportedNote {
                construct: "ref_siblings".into(),
                location: location.into(),
                message: "keywords beside $ref are outside the supported subset".into(),
            });
        }
        let resolved = match resolve_ref(&doc.value, r, ref_stack) {
            Ok(v) => v.clone(),
            Err(err) => {
                return Ok(unsupported_schema(
                    doc,
                    schema,
                    (location, physical),
                    match &err {
                        Error::Unsupported { construct, .. } => construct,
                        other => other.outcome_name(),
                    },
                    &err.to_string(),
                ));
            }
        };
        budget.charge(location, 0, &resolved, 0)?;
        working = resolved;
    }
    if schema_stack.iter().any(|item| item == &identity) {
        return Ok(unsupported_schema(
            doc,
            schema,
            (location, physical),
            "recursive_reference",
            &format!("schema cycle at {identity}"),
        ));
    }
    schema_stack.push(identity);

    let obj = match working.as_object() {
        Some(o) => o,
        None => {
            return Ok(unsupported_schema(
                doc,
                schema,
                (location, &here),
                "schema",
                "schema is not an object",
            ));
        }
    };

    for combinator in ["allOf", "oneOf", "anyOf", "not"] {
        if obj.contains_key(combinator) {
            unsupported.push(UnsupportedNote {
                construct: combinator.into(),
                location: location.into(),
                message:
                    "schema combinators are outside the supported subset and are not normalized away"
                        .into(),
            });
        }
    }
    if obj.contains_key("discriminator") {
        unsupported.push(UnsupportedNote {
            construct: "discriminator".into(),
            location: location.into(),
            message: "discriminators are outside the supported subset".into(),
        });
    }

    let (types, nullable, dialect_notes) = split_types(doc.dialect, obj, location);
    unsupported.extend(dialect_notes);

    let enum_values = match obj.get("enum") {
        Some(Value::Array(items)) => {
            if items.len() > MAX_ENUM {
                schema_stack.pop();
                return Err(Error::Bound {
                    kind: "enum".into(),
                    location: location.into(),
                    message: format!("enum has {} values; max is {MAX_ENUM}", items.len()),
                });
            }
            Some(items.clone())
        }
        Some(_) => {
            schema_stack.pop();
            return Err(Error::Parse {
                path: location.into(),
                message: "enum must be an array".into(),
            });
        }
        None => None,
    };

    let mut required = Vec::new();
    if let Some(Value::Array(items)) = obj.get("required") {
        for item in items {
            if let Some(name) = item.as_str() {
                required.push(name.to_string());
            } else {
                schema_stack.pop();
                return Err(Error::Parse {
                    path: location.into(),
                    message: "required entries must be strings".into(),
                });
            }
        }
    }

    let additional_properties = match obj.get("additionalProperties") {
        None | Some(Value::Bool(true)) => AdditionalProperties::Allowed,
        Some(Value::Bool(false)) => AdditionalProperties::Forbidden,
        Some(_) => {
            unsupported.push(UnsupportedNote {
                construct: "additionalProperties_schema".into(),
                location: location.into(),
                message: "additionalProperties as a nested schema is outside the supported subset"
                    .into(),
            });
            AdditionalProperties::Allowed
        }
    };

    let constraints = match read_constraints(obj, location, &mut unsupported, budget) {
        Ok(c) => c,
        Err(err) => {
            schema_stack.pop();
            return Err(err);
        }
    };

    let mut properties = BTreeMap::new();
    if let Some(Value::Object(props)) = obj.get("properties") {
        for (name, sub) in props {
            let child_loc = format!("{location}/properties/{}", pointer_escape(name));
            let child_physical = format!("{here}/properties/{}", pointer_escape(name));
            let mut child_refs = ref_stack.clone();
            let resolved = match resolve_schema_inner(
                doc,
                sub,
                (&child_loc, &child_physical),
                &mut child_refs,
                schema_stack,
                budget,
            ) {
                Ok(v) => v,
                Err(err) => {
                    schema_stack.pop();
                    return Err(err);
                }
            };
            properties.insert(name.clone(), resolved);
        }
    }

    let mut items = None;
    if types.iter().any(|t| t == "array") {
        match obj.get("items") {
            Some(item_schema) => {
                let mut child_refs = ref_stack.clone();
                match resolve_schema_inner(
                    doc,
                    item_schema,
                    (&format!("{location}/items"), &format!("{here}/items")),
                    &mut child_refs,
                    schema_stack,
                    budget,
                ) {
                    Ok(resolved) => items = Some(Box::new(resolved)),
                    Err(err) => {
                        schema_stack.pop();
                        return Err(err);
                    }
                }
            }
            None => unsupported.push(UnsupportedNote {
                construct: "array".into(),
                location: location.into(),
                message: "array schema without items is unsupported".into(),
            }),
        }
    }

    schema_stack.pop();
    Ok(ResolvedSchema {
        location: location.into(),
        physical: here,
        dialect: doc.dialect,
        nullable,
        types,
        enum_values,
        required,
        properties,
        additional_properties,
        items,
        unsupported,
        constraints,
        original: working,
    })
}

fn unsupported_schema(
    doc: &OpenApiDoc,
    schema: &Value,
    (location, physical): (&str, &str),
    construct: &str,
    message: &str,
) -> ResolvedSchema {
    ResolvedSchema {
        location: location.into(),
        physical: physical.into(),
        dialect: doc.dialect,
        nullable: false,
        types: Vec::new(),
        enum_values: None,
        required: Vec::new(),
        properties: BTreeMap::new(),
        additional_properties: AdditionalProperties::Allowed,
        items: None,
        unsupported: vec![UnsupportedNote {
            construct: construct.into(),
            location: location.into(),
            message: message.into(),
        }],
        constraints: Constraints::default(),
        original: schema.clone(),
    }
}

fn split_types(
    dialect: Dialect,
    obj: &Map<String, Value>,
    location: &str,
) -> (Vec<String>, bool, Vec<UnsupportedNote>) {
    let mut notes = Vec::new();
    let nullable_flag = obj
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match dialect {
        Dialect::Openapi30 => {
            if let Some(Value::Array(_)) = obj.get("type") {
                notes.push(UnsupportedNote {
                    construct: "type_array".into(),
                    location: location.into(),
                    message: "OpenAPI 3.0 uses a single type plus nullable; a type array is not normalized to 3.1"
                        .into(),
                });
            }
            // No `type` means any type: an empty list. `nullable` only adds null to a declared type.
            let mut types: Vec<String> = obj
                .get("type")
                .and_then(Value::as_str)
                .map(|t| vec![t.to_string()])
                .unwrap_or_default();
            if nullable_flag && !types.is_empty() && !types.iter().any(|t| t == "null") {
                types.push("null".into());
            }
            (types, nullable_flag, notes)
        }
        Dialect::Openapi31 => {
            if obj.get("nullable").is_some() {
                notes.push(UnsupportedNote {
                    construct: "nullable".into(),
                    location: location.into(),
                    message: "OpenAPI 3.1 nullable is a 3.0 construct and is not rewritten".into(),
                });
            }
            let types = match obj.get("type") {
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                // No `type` means any type: an empty list.
                _ => Vec::new(),
            };
            let nullable = types.iter().any(|t| t == "null");
            (types, nullable, notes)
        }
    }
}

pub fn validate_instance(schema: &ResolvedSchema, instance: &Value, location: &str) -> Result<()> {
    if schema.is_unsupported() {
        // The first note anywhere below, so a nested construct is named.
        let note = schema
            .all_unsupported()
            .into_iter()
            .next()
            .unwrap_or(UnsupportedNote {
                construct: "schema".into(),
                location: location.into(),
                message: "unsupported schema".into(),
            });
        return Err(Error::Unsupported {
            construct: note.construct,
            location: note.location,
            message: note.message,
        });
    }
    if instance.is_null() {
        if schema.types.is_empty() || schema.nullable || schema.types.iter().any(|t| t == "null") {
            return Ok(());
        }
        return Err(Error::SchemaValidation {
            location: location.into(),
            message: "null is not allowed".into(),
        });
    }
    let type_ok = schema.types.is_empty()
        || schema.types.iter().any(|t| match t.as_str() {
            "object" => instance.is_object(),
            "string" => instance.is_string(),
            "integer" => is_integer_value(instance),
            "number" => instance.is_number(),
            "boolean" => instance.is_boolean(),
            "null" => instance.is_null(),
            "array" => instance.is_array(),
            _ => false,
        });
    if !type_ok {
        return Err(Error::Malformed {
            kind: "type".into(),
            field: location.into(),
            location: location.into(),
            message: format!(
                "value type does not match schema types {}",
                schema.types.join("|")
            ),
        });
    }
    if let Some(enums) = &schema.enum_values {
        if !enums.iter().any(|allowed| json_equal(allowed, instance)) {
            return Err(Error::UnknownEnum {
                field: location.into(),
                value: instance.to_string(),
                location: location.into(),
            });
        }
    }
    if let Value::Object(map) = instance {
        for req in &schema.required {
            if !map.contains_key(req) {
                return Err(Error::MissingField {
                    field: req.clone(),
                    location: location.into(),
                });
            }
        }
        if schema.additional_properties == AdditionalProperties::Forbidden {
            for key in map.keys() {
                if !schema.properties.contains_key(key) {
                    return Err(Error::SchemaValidation {
                        location: format!("{location}/{key}"),
                        message: "additional property is forbidden".into(),
                    });
                }
            }
        }
        for (key, child) in &schema.properties {
            if let Some(value) = map.get(key) {
                validate_instance(child, value, &format!("{location}/{key}"))?;
            }
        }
    }
    check_constraints(schema, instance, location)?;
    if let Value::Array(items) = instance {
        if items.len() > MAX_ARRAY {
            return Err(Error::Bound {
                kind: "array".into(),
                location: location.into(),
                message: format!("array longer than {MAX_ARRAY} items"),
            });
        }
        if let Some(item_schema) = &schema.items {
            for (i, item) in items.iter().enumerate() {
                validate_instance(item_schema, item, &format!("{location}/{i}"))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn three_one_null_type_is_nullable() {
        let doc = OpenApiDoc {
            path: "mem.yaml".into(),
            line_index: None,
            digest: "sha256:x".into(),
            dialect: Dialect::Openapi31,
            openapi_version: "3.1.0".into(),
            title: "t".into(),
            api_version: "1".into(),
            value: json!({"openapi":"3.1.0","info":{"title":"t","version":"1"},"paths":{}}),
        };
        let schema = json!({"type":["string","null"]});
        let resolved = resolve_schema(&doc, &schema, "/s").unwrap();
        assert!(resolved.nullable);
        assert!(validate_instance(&resolved, &Value::Null, "/s").is_ok());
    }
}
