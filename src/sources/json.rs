use crate::{Category, Source};

/// An in-memory JSON [`Source`]. Reports [`Category::File`].
///
/// [`Config::add_json_str`](crate::Config::add_json_str) is the usual way to
/// add one; construct `JsonStr` directly only for
/// [`Config::add_source`](crate::Config::add_source).
///
/// ```
/// use provcfg::{Category, Config, Configurable};
/// use provcfg::sources::JsonStr;
///
/// # #[derive(Configurable, serde::Deserialize, Clone, Default)]
/// # struct Settings { host: String, port: u16 }
/// let settings = Config::new()
///     .add_source(JsonStr::new("app.json", r#"{ "host": "localhost", "port": 8080 }"#))
///     .build::<SettingsProv>()
///     .unwrap();
///
/// assert_eq!(settings.port.value(), &8080);
/// assert_eq!(settings.port.source().category(), Category::File);
/// ```
pub struct JsonStr {
    name: String,
    bytes: String,
}

impl JsonStr {
    /// Create a JSON source. `name` is a human-readable label used in error
    /// messages; `json` is the document text.
    pub fn new(name: impl Into<String>, json: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bytes: json.into(),
        }
    }
}

impl Source for JsonStr {
    fn name(&self) -> &str {
        &self.name
    }

    fn category(&self) -> Category {
        Category::File
    }

    fn deserialize(
        &self,
        seed: &mut dyn for<'de> FnMut(
            &mut dyn erased_serde::Deserializer<'de>,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        let mut json_de = serde_json::Deserializer::from_str(&self.bytes);
        let mut erased = <dyn erased_serde::Deserializer>::erase(&mut json_de);
        seed(&mut erased)
    }
}
