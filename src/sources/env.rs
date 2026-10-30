use std::collections::{BTreeMap, HashSet};

use serde::de::value::{Error as DeError, MapDeserializer, SeqDeserializer};
use serde::de::{self, Error as _, IntoDeserializer, Visitor};

use crate::{Category, Source};

/// Reads configuration values from process environment variables.
///
/// All variables starting with `<prefix>_` are collected into a tree that is
/// then handed to serde for deserialization. Each `__` in the remainder of the
/// variable name introduces a nested level; the resulting segments are
/// lowercased to match partial-struct field names.
///
/// Every value is kept as the raw string and converted to whatever type the
/// target field asks for:
///
/// - `String` (and `&str`, `char`, unit enum variants) get the value
///   verbatim. `123456`, `true`, `null` and `"quoted"` all arrive unchanged.
/// - Numeric and `bool` fields parse the value (`8000`, `-1`, `0.5`, `true`).
///   A value that does not parse is a deserialize error naming the value and
///   the expected type.
/// - `Option<T>` fields are `Some` as soon as the variable is set, with the
///   value converted as `T`: `Option<String>` gets the raw string (`null`
///   included), `Option<u16>` parses it. An env var cannot set an option to
///   `None`; leave the variable unset to fall back to lower layers instead.
/// - Self-describing deserialization (`deserialize_any`, used by
///   `#[serde(untagged)]` and `#[serde(flatten)]`) sees a string.
///
/// Use [`Self::with_list_keys`] to tell the source which dotted paths should
/// be split on `,`. This is required for CSV-encoded list fields like
/// `Vec<String>`; each element is then converted like a single value, so
/// `Vec<u16>` works as well.
///
/// ```
/// use provcfg::{Category, Config, Configurable};
/// use provcfg::sources::EnvSource;
///
/// # #[derive(Configurable, serde::Deserialize, Clone, Default)]
/// # struct Settings {
/// #     host: String,
/// #     #[configurable(nested)]
/// #     database: Database,
/// # }
/// # #[derive(Configurable, serde::Deserialize, Clone, Default)]
/// # struct Database { port: u16 }
/// # unsafe {
/// #     std::env::set_var("DEMO_HOST", "db.internal");
/// #     std::env::set_var("DEMO_DATABASE__PORT", "5432");
/// # }
/// // `DEMO_HOST` sets `host`; `DEMO_DATABASE__PORT` sets the nested `database.port`.
/// let settings = Config::new()
///     .add_source(EnvSource::new("env", "DEMO"))
///     .build::<SettingsProv>()
///     .unwrap();
///
/// assert_eq!(settings.host.value(), "db.internal");
/// assert_eq!(settings.database.port.value(), &5432);
/// assert_eq!(settings.database.port.source().category(), Category::Env);
/// ```
pub struct EnvSource {
    name: String,
    /// Pre-joined `"<prefix>_"` so we don't reformat it for every env var.
    prefix_with_sep: String,
    list_keys: HashSet<String>,
}

impl EnvSource {
    /// Create an environment source. `name` is a human-readable label used in
    /// error messages; only variables starting with `<prefix>_` are read.
    pub fn new(name: impl Into<String>, prefix: impl AsRef<str>) -> Self {
        Self {
            name: name.into(),
            prefix_with_sep: format!("{}_", prefix.as_ref()),
            list_keys: HashSet::new(),
        }
    }

    /// Mark a set of dotted paths (`"section.field"`) as CSV-encoded lists.
    /// Values for those paths are split on `,` and handed to serde as a
    /// sequence, so `MYAPP_SECTION__ITEMS=foo,bar` becomes
    /// `["foo", "bar"]` for `items: Vec<String>` fields.
    ///
    /// Whitespace around each element is trimmed. An empty value yields an
    /// empty list. Paths are matched case-insensitively (env var segments are
    /// always lowercased internally), so pass the path in either case.
    #[must_use]
    pub fn with_list_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.list_keys
            .extend(keys.into_iter().map(|k| k.into().to_lowercase()));
        self
    }
}

impl Source for EnvSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn category(&self) -> Category {
        Category::Env
    }

    fn deserialize(
        &self,
        seed: &mut dyn for<'de> FnMut(
            &mut dyn erased_serde::Deserializer<'de>,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        // Sort by env var name so the collision policy is deterministic: a
        // shorter key (`PREFIX_A`) is always processed before a deeper key
        // (`PREFIX_A__B`), so the deeper key wins for parent/child conflicts.
        let mut vars: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with(&self.prefix_with_sep))
            .collect();
        vars.sort_by(|a, b| a.0.cmp(&b.0));

        let mut root = BTreeMap::new();
        for (key, value) in vars {
            let rest = &key[self.prefix_with_sep.len()..];
            let segments: Vec<String> = rest.split("__").map(str::to_lowercase).collect();
            let dotted = segments.join(".");
            let node = parse_env_value(value, self.list_keys.contains(&dotted));
            insert_nested(&mut root, &segments, node);
        }
        let mut erased = <dyn erased_serde::Deserializer>::erase(EnvNode::Map(root));
        seed(&mut erased)
    }
}

/// Env values as collected from the process environment. Leaves stay raw
/// strings; [`EnvNode`]'s `Deserializer` impl converts them on demand into the
/// type the target field requests.
#[derive(Debug)]
enum EnvNode {
    Str(String),
    List(Vec<String>),
    Map(BTreeMap<String, EnvNode>),
}

fn parse_env_value(value: String, is_list_key: bool) -> EnvNode {
    if !is_list_key {
        return EnvNode::Str(value);
    }
    if value.is_empty() {
        return EnvNode::List(Vec::new());
    }
    EnvNode::List(value.split(',').map(|s| s.trim().to_string()).collect())
}

/// When a path element collides with an existing non-map value, the later
/// write wins. This matches what most env-driven configs do.
fn insert_nested(root: &mut BTreeMap<String, EnvNode>, path: &[String], value: EnvNode) {
    let Some((head, tail)) = path.split_first() else {
        return;
    };
    if tail.is_empty() {
        root.insert(head.clone(), value);
        return;
    }
    let entry = root
        .entry(head.clone())
        .or_insert_with(|| EnvNode::Map(BTreeMap::new()));
    if !matches!(entry, EnvNode::Map(_)) {
        *entry = EnvNode::Map(BTreeMap::new());
    }
    if let EnvNode::Map(child) = entry {
        insert_nested(child, tail, value);
    }
}

impl<'de> IntoDeserializer<'de, DeError> for EnvNode {
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
                    EnvNode::Str(s) => match s.trim().parse::<$ty>() {
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

impl<'de> de::Deserializer<'de> for EnvNode {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self {
            EnvNode::Str(s) => visitor.visit_string(s),
            EnvNode::List(items) => {
                let mut seq = SeqDeserializer::new(items.into_iter().map(EnvNode::Str));
                let value = visitor.visit_seq(&mut seq)?;
                seq.end()?;
                Ok(value)
            }
            EnvNode::Map(map) => {
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

    /// A set variable is always `Some`; see the [`EnvSource`] docs.
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
            EnvNode::Str(s) => visitor.visit_enum(s.into_deserializer()),
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
            let s = String::deserialize(EnvNode::Str(raw.to_string())).unwrap();
            assert_eq!(s, raw);
        }
    }

    #[test]
    fn string_leaf_is_parsed_for_scalars() {
        assert_eq!(u64::deserialize(EnvNode::Str("42".into())).unwrap(), 42);
        assert_eq!(i8::deserialize(EnvNode::Str("-7".into())).unwrap(), -7);
        assert!(bool::deserialize(EnvNode::Str("true".into())).unwrap());
        assert!(u8::deserialize(EnvNode::Str("256".into())).is_err());
        assert!(bool::deserialize(EnvNode::Str("yes".into())).is_err());
    }

    #[test]
    fn unit_enum_variant_from_string_leaf() {
        #[derive(Deserialize, Debug, PartialEq)]
        #[serde(rename_all = "lowercase")]
        enum Level {
            Info,
            Debug,
        }
        let level = Level::deserialize(EnvNode::Str("debug".into())).unwrap();
        assert_eq!(level, Level::Debug);
        assert!(Level::deserialize(EnvNode::Str("loud".into())).is_err());
    }
}
