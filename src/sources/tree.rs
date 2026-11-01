//! A tree of raw string values, shared by sources whose data arrives as flat
//! `path -> string` entries (environment variables, key/value database rows).
//! Leaves stay strings until serde asks for a concrete type, so `"8080"` can
//! become a `u16` and `"true"` a `bool` while `String` fields keep the text.

use std::collections::{BTreeMap, HashSet};

use serde::de::value::{Error as DeError, MapDeserializer, SeqDeserializer};
use serde::de::{self, Error as _, IntoDeserializer, Visitor};

#[derive(Debug)]
pub(crate) enum StrNode {
    Str(String),
    List(Vec<String>),
    Map(BTreeMap<String, StrNode>),
}

impl StrNode {
    /// Build a map tree from `(path segments, raw value)` entries. Values whose
    /// dotted path is in `list_keys` are split on `,`.
    ///
    /// Entries are stably sorted by path first, so a parent path is always
    /// inserted before its children and a deeper key wins a parent/child
    /// collision regardless of input order. For identical paths the later entry
    /// wins.
    pub(crate) fn from_entries(
        mut entries: Vec<(Vec<String>, String)>,
        list_keys: &HashSet<String>,
    ) -> Self {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut root = BTreeMap::new();
        for (segments, value) in entries {
            let node = Self::leaf(value, list_keys.contains(&segments.join(".")));
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

    fn leaf(value: String, is_list: bool) -> Self {
        if !is_list {
            return StrNode::Str(value);
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

/// Parse a string leaf with `FromStr` and hand the result to the visitor.
/// Non-string nodes fall through to `deserialize_any`, which produces serde's
/// usual "invalid type" error.
macro_rules! deserialize_parsed {
    ($($method:ident => $visit:ident: $ty:ty),* $(,)?) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
                match self {
                    StrNode::Str(s) => match s.trim().parse::<$ty>() {
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
            StrNode::Str(s) => visitor.visit_string(s),
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

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        match self {
            StrNode::Str(s) => visitor.visit_enum(s.into_deserializer()),
            other => other.deserialize_any(visitor),
        }
    }

    serde::forward_to_deserialize_any! {
        char str string bytes byte_buf unit unit_struct seq tuple tuple_struct
        map struct identifier ignored_any
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
