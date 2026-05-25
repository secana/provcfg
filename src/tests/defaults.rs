use super::fixtures::*;
use crate::*;

#[test]
fn build_with_no_sources_uses_defaults() {
    let config = Config::new();
    let db = config.build::<DatabaseConfigProv>().unwrap();

    assert_eq!(db.host.value(), "");
    assert_eq!(db.host.source().category(), Category::Default);
    assert_eq!(db.host.source().name(), "default");

    assert_eq!(db.port.value(), &0);
    assert_eq!(db.port.source().category(), Category::Default);
}

#[cfg(feature = "json")]
#[test]
fn unset_field_falls_back_to_default_category() {
    let host_only = r#"{ "host": "db.example" }"#;

    let config = Config::new().add_json_str("host.json", host_only);

    let db = config.build::<DatabaseConfigProv>().unwrap();

    assert_eq!(db.host.value(), "db.example");
    assert_eq!(db.host.source().category(), Category::File);

    assert_eq!(db.port.value(), &0);
    assert_eq!(db.port.source().category(), Category::Default);
}

#[cfg(feature = "json")]
#[test]
fn layered_sources_each_supply_subset_of_fields() {
    let host_only = r#"{ "host": "db.example" }"#;
    let port_only = r#"{ "port": 9000 }"#;

    let config = Config::new()
        .add_json_str("host.json", host_only)
        .add_json_str("port.json", port_only);

    let db = config.build::<DatabaseConfigProv>().unwrap();

    assert_eq!(db.host.value(), "db.example");
    assert_eq!(db.host.source().name(), "host.json");
    assert_eq!(db.port.value(), &9000);
    assert_eq!(db.port.source().name(), "port.json");
}

#[cfg(feature = "toml")]
#[test]
fn nested_struct_carries_provenance_per_leaf() {
    let toml = r#"
name = "my-app"
[database]
host = "db.example"
"#;

    let config = Config::new().add_toml_str("app.toml", toml);

    let app = config.build::<AppConfigProv>().unwrap();

    assert_eq!(app.name.value(), "my-app");
    assert_eq!(app.name.source().category(), Category::File);

    assert_eq!(app.database.host.value(), "db.example");
    assert_eq!(app.database.host.source().category(), Category::File);

    assert_eq!(app.database.port.value(), &0);
    assert_eq!(app.database.port.source().category(), Category::Default);
}

#[cfg(feature = "toml")]
#[test]
fn sources_map_flattens_provenance_to_dotted_paths() {
    let toml = r#"
name = "my-app"
[database]
host = "db.example"
"#;

    let config = Config::new().add_toml_str("k.toml", toml);
    let app = config.build::<AppConfigProv>().unwrap();

    let map = app.sources_map();
    assert_eq!(map.get("name"), Some(&Category::File));
    assert_eq!(map.get("database.host"), Some(&Category::File));
    assert_eq!(map.get("database.port"), Some(&Category::Default));
}

#[cfg(all(feature = "toml", feature = "env", feature = "cli"))]
#[test]
fn full_layered_stack_defaults_toml_env_cli() {
    let prefix = "PROVCFG_TEST_FULL_STACK";
    unsafe {
        std::env::set_var(format!("{prefix}_HOST"), "env.example");
    }

    let cli_partial = DatabaseConfigPartial {
        host: None,
        port: Some(3),
    };

    let config = Config::new()
        .add_toml_str("app.toml", "host = \"toml.example\"\nport = 1\n")
        .add_env(prefix)
        .add_cli(cli_partial);

    let db = config.build::<DatabaseConfigProv>().unwrap();

    assert_eq!(db.host.value(), "env.example");
    assert_eq!(db.host.source().category(), Category::Env);
    assert_eq!(db.port.value(), &3);
    assert_eq!(db.port.source().category(), Category::Cli);

    let map = db.sources_map();
    assert_eq!(map.get("host"), Some(&Category::Env));
    assert_eq!(map.get("port"), Some(&Category::Cli));

    unsafe {
        std::env::remove_var(format!("{prefix}_HOST"));
    }
}
