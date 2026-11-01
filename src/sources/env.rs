use std::collections::HashSet;

use super::tree::StrNode;
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
        // Sort by env var name so variables that lowercase to the same path
        // resolve deterministically: the later name wins.
        let mut vars: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with(&self.prefix_with_sep))
            .collect();
        vars.sort_by(|a, b| a.0.cmp(&b.0));

        let entries = vars
            .into_iter()
            .map(|(key, value)| {
                let rest = &key[self.prefix_with_sep.len()..];
                (rest.split("__").map(str::to_lowercase).collect(), value)
            })
            .collect();
        StrNode::from_entries(entries, &self.list_keys).feed(seed)
    }
}
