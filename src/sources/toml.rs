use crate::{Category, Source};

/// An in-memory TOML [`Source`]. Reports [`Category::File`].
///
/// [`Config::add_toml_str`](crate::Config::add_toml_str) is the usual way to
/// add one; construct `TomlStr` directly only for
/// [`Config::add_source`](crate::Config::add_source).
///
/// ```
/// use provcfg::{Category, Config, Configurable};
/// use provcfg::sources::TomlStr;
///
/// # #[derive(Configurable, serde::Deserialize, Clone, Default)]
/// # struct Settings { host: String, port: u16 }
/// let settings = Config::new()
///     .add_source(TomlStr::new("app.toml", "host = \"localhost\"\nport = 8080"))
///     .build::<SettingsProv>()
///     .unwrap();
///
/// assert_eq!(settings.host.value(), "localhost");
/// assert_eq!(settings.host.source().category(), Category::File);
/// ```
pub struct TomlStr {
    name: String,
    bytes: String,
}

impl TomlStr {
    /// Create a TOML source. `name` is a human-readable label used in error
    /// messages; `toml` is the document text.
    pub fn new(name: impl Into<String>, toml: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bytes: toml.into(),
        }
    }
}

impl Source for TomlStr {
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
        let toml_de = toml::Deserializer::parse(&self.bytes)
            .map_err(<erased_serde::Error as serde::de::Error>::custom)?;
        let mut erased = <dyn erased_serde::Deserializer>::erase(toml_de);
        seed(&mut erased)
    }
}
