//! Built-in [`Source`](crate::Source) implementations, each behind a feature:
//!
//! - `JsonStr`: an in-memory JSON document (`json` feature).
//! - `TomlStr`: an in-memory TOML document (`toml` feature).
//! - `EnvSource`: process environment variables (`env` feature, on by default).
//! - `CliSource`: a pre-built `*Partial`, e.g. from a CLI parser (`cli` feature).
//!
//! The [`Config`](crate::Config) `add_*` helpers wrap these. Construct them
//! directly only when reaching for [`Config::add_source`](crate::Config::add_source).

#[cfg(feature = "json")]
mod json;
#[cfg(feature = "json")]
pub use json::JsonStr;

#[cfg(feature = "toml")]
mod toml;
#[cfg(feature = "toml")]
pub use toml::TomlStr;

#[cfg(feature = "env")]
mod env;
#[cfg(feature = "env")]
pub use env::EnvSource;

#[cfg(feature = "cli")]
mod cli;
#[cfg(feature = "cli")]
pub use cli::CliSource;
