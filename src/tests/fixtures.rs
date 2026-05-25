use crate as provcfg;
use crate::Configurable;

#[derive(serde::Deserialize, Clone, Default, Configurable)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
}

#[derive(serde::Deserialize, Clone, Default, Configurable)]
pub struct AppConfig {
    #[configurable(nested)]
    pub database: DatabaseConfig,
    pub name: String,
}

#[derive(serde::Deserialize, Clone, Default, Configurable)]
pub struct WithSecret {
    pub name: String,
    #[configurable(secret)]
    pub password: String,
}

#[derive(serde::Deserialize, Clone, Default, Configurable)]
pub struct Renamed {
    #[configurable(rename = "HOSTNAME")]
    pub host: String,
}

#[derive(serde::Deserialize, Clone, Default, Configurable)]
pub struct WithList {
    #[configurable(env_list)]
    pub items: Vec<String>,
}

/// A field that lives on the user struct but is invisible to the
/// `Configurable` machinery. Useful for runtime state attached to a config
/// type that should not participate in provenance.
#[derive(serde::Deserialize, Clone, Default, Configurable)]
pub struct WithSkipped {
    pub host: String,
    #[configurable(skip)]
    #[serde(skip)]
    pub runtime_state: Vec<String>,
}

/// Edge case: every field is `#[configurable(skip)]`. Exercises the macro's
/// "no tracked leaves" path for the generated `From<&OnlySkippedProv>`.
#[derive(serde::Deserialize, Clone, Default, Configurable)]
pub struct OnlySkipped {
    #[configurable(skip)]
    #[serde(skip)]
    pub runtime_state: Vec<String>,
}
