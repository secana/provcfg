use std::collections::HashSet;

use crate::{Category, Source};

/// Reads configuration values from process environment variables.
///
/// All variables starting with `<prefix>_` are collected and turned into a
/// JSON object that is then handed to serde for deserialization. Each `__`
/// in the remainder of the variable name introduces a nested level; the
/// resulting segments are lowercased to match partial-struct field names.
///
/// Each value is first parsed as JSON; if that fails it is kept as a string.
/// This lets the source supply `u16` / `bool` / array values without macro
/// support, while still tolerating shell-style bare strings.
///
/// Use [`Self::with_list_keys`] to tell the source which dotted paths should
/// be split on `,`. This is required for CSV-encoded list fields like
/// `Vec<String>` that the generic JSON-fallback can't infer from the string
/// alone.
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
    /// Values for those paths are split on `,` and wrapped in a JSON array
    /// before serde deserialization, so `MYAPP_SECTION__ITEMS=foo,bar` becomes
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

        let mut root = serde_json::Map::new();
        for (key, value) in vars {
            let rest = &key[self.prefix_with_sep.len()..];
            let segments: Vec<String> = rest.split("__").map(str::to_lowercase).collect();
            let dotted = segments.join(".");
            let parsed = parse_env_value(value, self.list_keys.contains(&dotted));
            insert_nested(&mut root, &segments, parsed);
        }
        let value = serde_json::Value::Object(root);
        let mut erased = <dyn erased_serde::Deserializer>::erase(&value);
        seed(&mut erased)
    }
}

fn parse_env_value(value: String, is_list_key: bool) -> serde_json::Value {
    if is_list_key {
        if value.is_empty() {
            return serde_json::Value::Array(Vec::new());
        }
        return serde_json::Value::Array(
            value
                .split(',')
                .map(|s| serde_json::Value::String(s.trim().to_string()))
                .collect(),
        );
    }
    serde_json::from_str::<serde_json::Value>(&value).unwrap_or(serde_json::Value::String(value))
}

/// When a path element collides with an existing non-object value, the later
/// write wins. This matches what most env-driven configs do.
fn insert_nested(
    root: &mut serde_json::Map<String, serde_json::Value>,
    path: &[String],
    value: serde_json::Value,
) {
    if path.is_empty() {
        return;
    }
    if path.len() == 1 {
        root.insert(path[0].clone(), value);
        return;
    }
    let head = &path[0];
    let entry = root
        .entry(head.clone())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        *entry = serde_json::Value::Object(serde_json::Map::new());
    }
    let child = entry
        .as_object_mut()
        .expect("entry was just coerced to Object above");
    insert_nested(child, &path[1..], value);
}
