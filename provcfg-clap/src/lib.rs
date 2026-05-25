//! Clap integration for [`provcfg`](https://docs.rs/provcfg).
//!
//! Derive [`ClapArgs`] alongside `provcfg::Configurable` to generate a
//! `<Name>Args` struct (a `clap::Args`) plus a `From<&<Name>Args> for
//! <Name>Partial` impl. A clap-parsed command line then flows into a provcfg
//! CLI source without a hand-written per-field conversion.
//!
//! ```
//! use clap::Parser;
//! use provcfg::{Category, Config, Configurable};
//! use provcfg_clap::ClapArgs;
//!
//! #[derive(Configurable, ClapArgs, Clone, Default, serde::Deserialize, serde::Serialize)]
//! #[configurable(clap_prefix = "registry")]
//! struct Registry {
//!     data_dir: String, // exposed as --registry-data-dir
//!     port: u16,         // exposed as --registry-port
//! }
//!
//! #[derive(Parser)]
//! struct Cli {
//!     #[command(flatten)]
//!     registry: RegistryArgs,
//! }
//!
//! let cli = Cli::parse_from(["app", "--registry-data-dir", "/var/lib/reg"]);
//!
//! // The generated `From<&RegistryArgs>` builds the provcfg partial for us.
//! let partial: RegistryPartial = (&cli.registry).into();
//! let registry = Config::new().add_cli(partial).build::<RegistryProv>().unwrap();
//!
//! assert_eq!(registry.data_dir.value(), "/var/lib/reg");
//! assert_eq!(registry.data_dir.source().category(), Category::Cli);
//! // `--registry-port` was not passed, so `port` keeps its compiled-in default.
//! assert_eq!(registry.port.source().category(), Category::Default);
//! ```
//!
//! The `ClapArgs` derive:
//!
//! - generates `<Name>Args` with each leaf wrapped in `Option<T>` and an
//!   `#[arg(long = "<prefix>-<field>")]` auto-derived from the field name;
//! - forwards any user-written `#[arg(...)]` attributes; a user-supplied
//!   `long = "..."` overrides the auto-derived one;
//! - for `#[configurable(nested)]` fields emits `#[command(flatten)]` into the
//!   nested type's own `Args` struct (which must also derive `ClapArgs`);
//! - omits `#[configurable(skip)]` fields from both the Args struct and the
//!   `From` impl.

/// Optional convenience re-export. The `ClapArgs` derive emits `::clap::Args`,
/// so consumers must have `clap` as a direct dependency regardless; this
/// re-export is only here for callers that prefer to reach for clap through
/// `provcfg_clap::clap`.
pub use clap;
pub use provcfg_clap_macros::ClapArgs;
