//! A tree of raw string values, shared by sources whose data arrives as flat
//! `path -> string` entries (environment variables, key/value database rows).
//! Leaves stay strings until serde asks for a concrete type, so `"8080"` can
//! become a `u16` and `"true"` a `bool` while `String` fields keep the text.
//! Sources that opt into JSON values (`json` feature) also turn a leaf into a
//! list, map, struct or enum with data by parsing it as JSON.

use std::collections::{BTreeMap, HashSet};

use serde::de::value::{Error as DeError, MapDeserializer, SeqDeserializer};
use serde::de::{self, Error as _, IntoDeserializer, Visitor};

#[derive(Debug)]
pub(crate) enum StrNode {
    Str(String),
    /// A string leaf that is parsed as JSON when the target is a list, map,
    /// struct or enum with data, and used like [`StrNode::Str`] otherwise.
    #[cfg_attr(not(feature = "json"), allow(dead_code))]
    Json(String),
    List(Vec<String>),
    Map(BTreeMap<String, StrNode>),
}

impl StrNode {
    /// Build a map tree from `(path segments, raw value)` entries. Values whose
    /// dotted path is in `list_keys` are split on `,`; with `json_values` the
    /// other values become [`StrNode::Json`].
    ///
    /// Entries are stably sorted by path first, so a parent path is always
    /// inserted before its children and a deeper key wins a parent/child
    /// collision regardless of input order. For identical paths the later entry
    /// wins.
    pub(crate) fn from_entries(
        mut entries: Vec<(Vec<String>, String)>,
        list_keys: &HashSet<String>,
        json_values: bool,
    ) -> Self {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut root = BTreeMap::new();
        for (segments, value) in entries {
            let is_list = list_keys.contains(&segments.join("."));
            let node = Self::leaf(value, is_list, json_values);
            insert_nested(&mut root, &segments, node);
        }
        StrNode::Map(root)
    }

    /// Hand the tree to a [`Source::deserialize`](crate::Source::deserialize)
    /// seed.
    pub(crate) fn feed(
        self,
        seed: &mut dyn for<'de> FnMut(
            &mut dyn erased_serde::Deserializer<'de>,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        let mut erased = <dyn erased_serde::Deserializer>::erase(self);
        seed(&mut erased)
    }

    fn leaf(value: String, is_list: bool, json_values: bool) -> Self {
        if !is_list {
            return if json_values {
                StrNode::Json(value)
            } else {
                StrNode::Str(value)
            };
        }
        if value.is_empty() {
            return StrNode::List(Vec::new());
        }
        StrNode::List(value.split(',').map(|s| s.trim().to_string()).collect())
    }
}

/// When a path element collides with an existing non-map value, the later
/// write wins.
fn insert_nested(root: &mut BTreeMap<String, StrNode>, path: &[String], value: StrNode) {
    let Some((head, tail)) = path.split_first() else {
        return;
    };
    if tail.is_empty() {
        root.insert(head.clone(), value);
        return;
    }
    let entry = root
        .entry(head.clone())
        .or_insert_with(|| StrNode::Map(BTreeMap::new()));
    if !matches!(entry, StrNode::Map(_)) {
        *entry = StrNode::Map(BTreeMap::new());
    }
    if let StrNode::Map(child) = entry {
        insert_nested(child, tail, value);
    }
}

impl<'de> IntoDeserializer<'de, DeError> for StrNode {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

/// A [`StrNode::Json`] leaf parsed as JSON and handed to the same method of
/// the parsed value; every other node goes to `deserialize_any`, which gives
/// serde's usual "invalid type" error for a string.
macro_rules! deserialize_structured {
    ($($method:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {
        $(
            #[cfg_attr(not(feature = "json"), allow(unused_variables))]
            fn $method<V: Visitor<'de>>(self, $($arg: $ty,)* visitor: V) -> Result<V::Value, DeError> {
                #[cfg(feature = "json")]
                if let StrNode::Json(s) = self {
                    return de::Deserializer::$method(parse_json(&s)?, $($arg,)* visitor)
                        .map_err(DeError::custom);
                }
                self.deserialize_any(visitor)
            }
        )*
    };
}

#[cfg(feature = "json")]
fn parse_json(text: &str) -> Result<serde_json::Value, DeError> {
    serde_json::from_str(text)
        .map_err(|e| DeError::custom(format!("invalid JSON value {text:?}: {e}")))
}

/// Parse a string leaf with `FromStr` and hand the result to the visitor.
/// Non-string nodes fall through to `deserialize_any`, which produces serde's
/// usual "invalid type" error.
macro_rules! deserialize_parsed {
    ($($method:ident => $visit:ident: $ty:ty),* $(,)?) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
                match self {
                    StrNode::Str(s) | StrNode::Json(s) => match s.trim().parse::<$ty>() {
                        Ok(v) => visitor.$visit(v),
                        Err(e) => Err(DeError::custom(format!(
                            "invalid value {s:?}, expected {}: {e}",
                            stringify!($ty)
                        ))),
                    },
                    other => other.deserialize_any(visitor),
                }
            }
        )*
    };
}

impl<'de> de::Deserializer<'de> for StrNode {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self {
            StrNode::Str(s) | StrNode::Json(s) => visitor.visit_string(s),
            StrNode::List(items) => {
                let mut seq = SeqDeserializer::new(items.into_iter().map(StrNode::Str));
                let value = visitor.visit_seq(&mut seq)?;
                seq.end()?;
                Ok(value)
            }
            StrNode::Map(map) => {
                let mut map = MapDeserializer::new(map.into_iter());
                let value = visitor.visit_map(&mut map)?;
                map.end()?;
                Ok(value)
            }
        }
    }

    deserialize_parsed! {
        deserialize_bool => visit_bool: bool,
        deserialize_i8 => visit_i8: i8,
        deserialize_i16 => visit_i16: i16,
        deserialize_i32 => visit_i32: i32,
        deserialize_i64 => visit_i64: i64,
        deserialize_i128 => visit_i128: i128,
        deserialize_u8 => visit_u8: u8,
        deserialize_u16 => visit_u16: u16,
        deserialize_u32 => visit_u32: u32,
        deserialize_u64 => visit_u64: u64,
        deserialize_u128 => visit_u128: u128,
        deserialize_f32 => visit_f32: f32,
        deserialize_f64 => visit_f64: f64,
    }

    /// A present entry is always `Some`; absence is the only way to get `None`.
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_newtype_struct(self)
    }

    #[cfg_attr(not(feature = "json"), allow(unused_variables))]
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        match self {
            // An enum with data is a JSON object such as `{"Tcp": {"port": 80}}`.
            #[cfg(feature = "json")]
            StrNode::Json(s) if s.trim_start().starts_with('{') => {
                de::Deserializer::deserialize_enum(parse_json(&s)?, name, variants, visitor)
                    .map_err(DeError::custom)
            }
            StrNode::Str(s) | StrNode::Json(s) => visitor.visit_enum(s.into_deserializer()),
            other => other.deserialize_any(visitor),
        }
    }

    deserialize_structured! {
        deserialize_seq(),
        deserialize_tuple(len: usize),
        deserialize_tuple_struct(name: &'static str, len: usize),
        deserialize_map(),
        deserialize_struct(name: &'static str, fields: &'static [&'static str]),
    }

    serde::forward_to_deserialize_any! {
        char str string bytes byte_buf unit unit_struct identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[test]
    fn string_leaf_is_returned_verbatim() {
        for raw in ["123456", "true", "null", r#""abc""#, "", " padded "] {
            let s = String::deserialize(StrNode::Str(raw.to_string())).unwrap();
            assert_eq!(s, raw);
        }
    }

    #[test]
    fn string_leaf_is_parsed_for_scalars() {
        assert_eq!(u64::deserialize(StrNode::Str("42".into())).unwrap(), 42);
        assert_eq!(i8::deserialize(StrNode::Str("-7".into())).unwrap(), -7);
        assert!(bool::deserialize(StrNode::Str("true".into())).unwrap());
        assert!(u8::deserialize(StrNode::Str("256".into())).is_err());
        assert!(bool::deserialize(StrNode::Str("yes".into())).is_err());
    }

    #[test]
    fn unit_enum_variant_from_string_leaf() {
        #[derive(Deserialize, Debug, PartialEq)]
        #[serde(rename_all = "lowercase")]
        enum Level {
            Info,
            Debug,
        }
        let level = Level::deserialize(StrNode::Str("debug".into())).unwrap();
        assert_eq!(level, Level::Debug);
        assert!(Level::deserialize(StrNode::Str("loud".into())).is_err());
    }
}
