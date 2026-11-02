//! JSON and YAML reading that refuses duplicate object keys.
//!
//! `serde_json::Value` keeps the last of two equal keys and drops the first
//! without a word, so `{"id":"a","id":"b"}` reads as `{"id":"b"}`. Every JSON
//! input this tool reads goes through `from_str` instead, which fails naming
//! the key and its RFC 6901 pointer. YAML contracts go through `from_yaml_str`,
//! the same check on the YAML parser's events.

use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::parse::pointer_escape;

/// Parse one JSON document; a duplicate key anywhere is an error.
pub fn from_str(text: &str) -> serde_json::Result<Value> {
    let mut de = serde_json::Deserializer::from_str(text);
    let value = Strict {
        pointer: String::new(),
    }
    .deserialize(&mut de)?;
    de.end()?;
    Ok(value)
}

/// Parse one YAML document into JSON values; a duplicate key anywhere is an error.
pub fn from_yaml_str(text: &str) -> Result<Value, yaml_serde::Error> {
    Strict {
        pointer: String::new(),
    }
    .deserialize(yaml_serde::Deserializer::from_str(text))
}

struct Strict {
    pointer: String,
}

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Strict {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Number::from_f64(v).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(Strict {
            pointer: format!("{}/{}", self.pointer, items.len()),
        })? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let pointer = format!("{}/{}", self.pointer, pointer_escape(&key));
            if object.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "duplicate key `{key}` at {pointer}"
                )));
            }
            let value = map.next_value_seed(Strict { pointer })?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_keys_are_refused_at_any_depth_with_their_pointer() {
        let err = from_str(r#"{"a":1,"a":2}"#).unwrap_err().to_string();
        assert!(err.contains("duplicate key `a` at /a"), "{err}");
        let err = from_str(r#"{"x":[{"k~/":1,"k~/":2}]}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("at /x/0/k~0~1"), "{err}");
    }

    #[test]
    fn reads_the_same_values_as_serde_json() {
        let text = r#"{"s":"t","n":-3,"u":18446744073709551615,"f":1.5,"b":true,"z":null,"a":[1,{"k":"v"}]}"#;
        let expected: Value = serde_json::from_str(text).unwrap();
        assert_eq!(from_str(text).unwrap(), expected);
        assert!(from_str("{} trailing").is_err());
    }

    #[test]
    fn yaml_duplicate_keys_are_refused_and_values_match_the_plain_reader() {
        let err = from_yaml_str(
            "a:
  b: 1
  b: 2
",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("duplicate key `b` at /a/b"), "{err}");
        let text = "s: t
n: -3
f: 1.5
b: true
z: ~
i: {200: ok}
l: [1, {k: v}]
";
        let expected: Value = yaml_serde::from_str(text).unwrap();
        assert_eq!(from_yaml_str(text).unwrap(), expected);
    }
}
