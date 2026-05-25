//! End-to-end check that `ClapArgs` + `Configurable` interoperate. Parses a
//! synthetic command line, converts the resulting Args into the provcfg
//! Partial, builds a `Config` with the CLI source, and asserts both the
//! active values and their `Category::Cli` provenance.

use clap::Parser;
use provcfg::{Category, Config, Configurable};
use provcfg_clap::ClapArgs;

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, Configurable, ClapArgs)]
#[configurable(clap_prefix = "database")]
struct DatabaseConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, Configurable, ClapArgs)]
#[configurable(clap_prefix = "app")]
struct AppConfig {
    #[configurable(nested)]
    pub database: DatabaseConfig,
    pub name: String,
}

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    args: AppConfigArgs,
}

#[test]
fn clap_args_round_trip_into_partial_and_prov() {
    let cli = Cli::parse_from([
        "test-bin",
        "--app-name",
        "app-from-cli",
        "--database-host",
        "db.example",
        "--database-port",
        "5432",
    ]);

    // The macro-generated `From<&AppConfigArgs> for AppConfigPartial` does the
    // section-by-section copy automatically. No hand-written conversion.
    let partial: AppConfigPartial = (&cli.args).into();

    let config = Config::new().add_cli(partial);
    let app = config.build::<AppConfigProv>().unwrap();

    assert_eq!(app.name.value(), "app-from-cli");
    assert_eq!(app.name.source().category(), Category::Cli);
    assert_eq!(app.database.host.value(), "db.example");
    assert_eq!(app.database.host.source().category(), Category::Cli);
    assert_eq!(app.database.port.value(), &5432);
    assert_eq!(app.database.port.source().category(), Category::Cli);
}

#[test]
fn unset_cli_flags_keep_partial_fields_none() {
    // Only `--app-name` provided; everything else stays at defaults.
    let cli = Cli::parse_from(["test-bin", "--app-name", "only-name"]);
    let partial: AppConfigPartial = (&cli.args).into();

    let config = Config::new().add_cli(partial);
    let app = config.build::<AppConfigProv>().unwrap();

    assert_eq!(app.name.value(), "only-name");
    assert_eq!(app.name.source().category(), Category::Cli);

    // database fields not touched on the CLI → defaults layer wins.
    assert_eq!(app.database.host.value(), "");
    assert_eq!(app.database.host.source().category(), Category::Default);
    assert_eq!(app.database.port.value(), &0);
    assert_eq!(app.database.port.source().category(), Category::Default);
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, Configurable, ClapArgs)]
struct WithUserOverride {
    /// `#[arg(long = "...")]` from the user wins over the auto-derived flag.
    #[arg(long = "custom-name", short = 'n')]
    pub name: String,
}

#[derive(Parser)]
struct CliWithOverride {
    #[command(flatten)]
    args: WithUserOverrideArgs,
}

#[test]
fn user_supplied_long_flag_overrides_auto_derive() {
    let cli = CliWithOverride::parse_from(["test-bin", "--custom-name", "x"]);
    let partial: WithUserOverridePartial = (&cli.args).into();
    let config = Config::new().add_cli(partial);
    let cfg = config.build::<WithUserOverrideProv>().unwrap();
    assert_eq!(cfg.name.value(), "x");
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, Configurable, ClapArgs)]
struct WithSkipped {
    pub host: String,
    /// Lives on `WithSkipped` for application code, but is invisible to the
    /// `Configurable`/`ClapArgs` machinery thanks to `#[configurable(skip)]`.
    #[configurable(skip)]
    #[serde(skip)]
    #[allow(dead_code)]
    pub runtime_cache: u64,
}

#[derive(Parser)]
struct CliWithSkipped {
    #[command(flatten)]
    args: WithSkippedArgs,
}

#[test]
fn configurable_skip_field_is_absent_from_args_struct() {
    // The Args struct only has `host`. Trying to pass `--runtime-cache` would
    // fail; here we just verify the supported flag works.
    let cli = CliWithSkipped::parse_from(["test-bin", "--host", "ok"]);
    let partial: WithSkippedPartial = (&cli.args).into();
    assert_eq!(partial.host.as_deref(), Some("ok"));
}

// --- Regression: shared leaf names across prefixed sections ----------------

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, Configurable, ClapArgs)]
#[configurable(clap_prefix = "alpha")]
struct Alpha {
    /// Leaf name intentionally shared with `Beta::enabled`.
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, Configurable, ClapArgs)]
#[configurable(clap_prefix = "beta")]
struct Beta {
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, Configurable, ClapArgs)]
struct AppWithSharedLeaves {
    #[configurable(nested)]
    pub alpha: Alpha,
    #[configurable(nested)]
    pub beta: Beta,
}

#[derive(Parser)]
struct CliShared {
    #[command(flatten)]
    args: AppWithSharedLeavesArgs,
}

/// Without an auto-derived `id` matching the prefixed `long`, clap panics at
/// parser construction with "Argument names must be unique, but 'enabled' is
/// in use by more than one argument or group" whenever two sections share a
/// leaf name. Auto-deriving only `long` is not enough; clap's default `id`
/// comes from the field name, so the prefix has to flow into `id` too.
#[test]
fn shared_leaf_names_across_prefixed_sections_do_not_collide() {
    let cli = CliShared::parse_from(["test-bin", "--alpha-enabled=true", "--beta-enabled=false"]);
    let partial: AppWithSharedLeavesPartial = (&cli.args).into();

    assert_eq!(
        partial.alpha.and_then(|a| a.enabled),
        Some(true),
        "alpha.enabled should round-trip true"
    );
    assert_eq!(
        partial.beta.and_then(|b| b.enabled),
        Some(false),
        "beta.enabled should round-trip false"
    );
}
