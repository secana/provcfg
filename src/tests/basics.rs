use super::fixtures::*;
use crate::*;

#[cfg(feature = "json")]
#[test]
fn simple_provenance() {
    let json = r#"{ "host": "localhost", "port": 5432 }"#;

    let config = Config::new().add_json_str("Json Source", json);

    let db = config.build::<DatabaseConfigProv>().unwrap();

    assert_eq!(db.host.value(), "localhost");
    assert_eq!(db.host.source().name(), "Json Source");

    assert_eq!(db.port.value(), &5432);
    assert_eq!(db.port.source().name(), "Json Source");
}

#[test]
fn partial_from_ref_wraps_every_field_in_some() {
    let cfg = DatabaseConfig {
        host: "db.example".to_string(),
        port: 9000,
    };
    let partial: DatabaseConfigPartial = (&cfg).into();
    assert_eq!(partial.host.as_deref(), Some("db.example"));
    assert_eq!(partial.port, Some(9000));
}

#[test]
fn partial_from_default_uses_struct_defaults() {
    let partial: DatabaseConfigPartial = (&DatabaseConfig::default()).into();
    assert_eq!(partial.host.as_deref(), Some(""));
    assert_eq!(partial.port, Some(0));
}

#[cfg(feature = "toml")]
#[test]
fn prov_converts_into_plain_user_struct() {
    let toml = r#"
name = "my-app"
[database]
host = "db.example"
"#;

    let config = Config::new().add_toml_str("k.toml", toml);
    let app_prov = config.build::<AppConfigProv>().unwrap();

    // The macro-generated `From<&AppConfigProv> for AppConfig` takes each
    // leaf's active value. No serde round-trip.
    let app: AppConfig = (&app_prov).into();
    assert_eq!(app.name, "my-app");
    assert_eq!(app.database.host, "db.example");
    assert_eq!(app.database.port, 0); // unset → default
}

#[test]
fn prov_into_user_struct_fills_skipped_field_with_default() {
    let cfg_prov = Config::new().build::<WithSkippedProv>().unwrap();
    let cfg: WithSkipped = (&cfg_prov).into();
    // `runtime_state` is `#[configurable(skip)]`, so it is reconstructed via Default.
    assert_eq!(cfg.host, "");
    assert!(cfg.runtime_state.is_empty());
}

#[test]
fn all_skipped_struct_compiles_and_converts() {
    // Regression: a struct whose every field is `#[configurable(skip)]` must
    // still produce valid `From<&Prov>` code (no leading-comma syntax error).
    let cfg_prov = Config::new().build::<OnlySkippedProv>().unwrap();
    let cfg: OnlySkipped = (&cfg_prov).into();
    assert!(cfg.runtime_state.is_empty());
}

#[cfg(feature = "json")]
#[test]
fn history_retains_overridden_values_in_order() {
    let cfg = Config::new()
        .add_json_str("a.json", r#"{"host": "first"}"#)
        .add_json_str("b.json", r#"{"host": "second"}"#)
        .build::<DatabaseConfigProv>()
        .unwrap();

    let chain = cfg.host.history();
    assert_eq!(chain.len(), 3, "defaults + 2 sources");
    assert_eq!(chain[0].value, "");
    assert_eq!(chain[0].source.category(), Category::Default);
    assert_eq!(chain[1].value, "first");
    assert_eq!(chain[1].source.name(), "a.json");
    assert_eq!(chain[2].value, "second");
    assert_eq!(chain[2].source.name(), "b.json");
    // Active accessors agree with the last entry.
    assert_eq!(cfg.host.value(), "second");
}

#[cfg(feature = "json")]
#[test]
fn prov_struct_is_send_and_can_cross_thread_in_arc() {
    use std::sync::Arc;

    let config = Config::new().add_json_str("Json Source", r#"{ "host": "x", "port": 1 }"#);

    let cfg = Arc::new(config.build::<DatabaseConfigProv>().unwrap());
    let cfg2 = Arc::clone(&cfg);
    let host: String = std::thread::spawn(move || cfg2.host.value().clone())
        .join()
        .unwrap();
    assert_eq!(host, "x");
}
