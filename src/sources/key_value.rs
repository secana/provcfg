use std::collections::HashSet;

use super::tree::StrNode;
use crate::{Category, Source};

/// A [`Source`] built from flat `dotted.key -> string` entries, such as the
/// rows of a key/value settings table. Reports [`Category::Database`].
///
/// It holds no connection: load the entries first (the `provcfg-sea-orm`
/// crate does this for SQLite and PostgreSQL), then hand them over.
///
/// - Each `.` in a key introduces a nested level. Keys are matched verbatim,
///   like keys in a TOML or JSON file.
/// - Values stay strings until the target field asks for a type, exactly as
///   with [`EnvSource`](crate::sources::EnvSource): `String` fields get the
///   text unchanged, numbers and `bool` are parsed, and a present key makes an
///   `Option<T>` field `Some`.
/// - When a key and one of its children are both present (`database` and
///   `database.port`), the child wins regardless of entry order.
/// - With [`with_json_values`](Self::with_json_values) (`json` feature), a
///   value for a list, map, struct or enum with data is parsed as JSON, so one
///   entry can hold a whole `Vec<Host>`.
///
/// ```
/// use provcfg::{Category, Config, Configurable};
/// use provcfg::sources::KeyValueSource;
///
/// # #[derive(Configurable, serde::Deserialize, Clone, Default)]
/// # struct Settings {
/// #     #[configurable(nested)]
/// #     database: Database,
/// # }
/// # #[derive(Configurable, serde::Deserialize, Clone, Default)]
/// # struct Database { port: u16 }
/// let rows = [("database.port", "5432")];
///
/// let settings = Config::new()
///     .add_source(KeyValueSource::new("settings-db", rows))
///     .build::<SettingsProv>()
///     .unwrap();
///
/// assert_eq!(settings.database.port.value(), &5432);
/// assert_eq!(settings.database.port.source().category(), Category::Database);
/// ```
pub struct KeyValueSource {
    name: String,
    entries: Vec<(String, String)>,
    list_keys: HashSet<String>,
    json_values: bool,
}

impl KeyValueSource {
    /// Create a key/value source. `name` is a human-readable label used in
    /// error messages.
    pub fn new<I, K, V>(name: impl Into<String>, entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            name: name.into(),
            entries: entries
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            list_keys: HashSet::new(),
            json_values: false,
        }
    }

    /// Mark dotted keys whose values are comma-separated lists. Each element is
    /// trimmed and converted like a single value, so `Vec<u16>` works. An empty
    /// value yields an empty list. Fields marked `#[configurable(env_list)]`
    /// accept comma-separated values without this.
    #[must_use]
    pub fn with_list_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.list_keys.extend(keys.into_iter().map(Into::into));
        self
    }

    /// Parse a value as JSON when its field is a list, map, struct or enum
    /// with data, e.g. `[{"name": "a"}]` for a `Vec<Host>` or `{"Tcp": 80}`
    /// for an enum. Scalars, strings and unit enum variants stay plain text
    /// (`8080`, `debug`, not `"debug"`), and so does a field that deserializes
    /// itself from any type, such as an untagged enum. Without this, such a
    /// value is an "invalid type" error. List keys are still split on `,`.
    #[cfg(feature = "json")]
    #[must_use]
    pub fn with_json_values(mut self) -> Self {
        self.json_values = true;
        self
    }
}

impl Source for KeyValueSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn category(&self) -> Category {
        Category::Database
    }

    fn deserialize(
        &self,
        seed: &mut dyn for<'de> FnMut(
            &mut dyn erased_serde::Deserializer<'de>,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        let entries = self
            .entries
            .iter()
            .map(|(key, value)| (key.split('.').map(str::to_string).collect(), value.clone()))
            .collect();
        StrNode::from_entries(entries, &self.list_keys, self.json_values).feed(seed)
    }
}
