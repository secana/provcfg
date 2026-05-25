use super::fixtures::*;
use crate::*;

#[cfg(feature = "json")]
#[test]
fn json_source_has_file_category() {
    use crate::sources::JsonStr;

    let src = JsonStr::new("app.json", "{}");
    assert_eq!(src.category(), Category::File);
}

#[cfg(feature = "json")]
#[test]
fn malformed_json_error_display() {
    let config = Config::new().add_json_str("broken.json", "{ not valid json");

    let err = match config.build::<DatabaseConfigProv>() {
        Ok(_) => panic!("expected build to fail on malformed JSON"),
        Err(e) => e,
    };
    let msg = err.to_string();

    assert!(
        msg.contains("broken.json"),
        "error message should name the source, got: {msg}"
    );
    assert!(
        std::error::Error::source(&err).is_some(),
        "error should carry the underlying serde error as a source"
    );
}

#[cfg(feature = "env")]
#[test]
fn env_source_reads_flat_fields_with_env_category() {
    let prefix = "PROVCFG_TEST_STEP10";
    // SAFETY: tests use a unique prefix; no other test reads/writes these vars.
    unsafe {
        std::env::set_var(format!("{prefix}_HOST"), "env.example");
        std::env::set_var(format!("{prefix}_PORT"), "4242");
    }

    let config = Config::new().add_env(prefix);

    let db = config.build::<DatabaseConfigProv>().unwrap();

    assert_eq!(db.host.value(), "env.example");
    assert_eq!(db.host.source().category(), Category::Env);
    assert_eq!(db.port.value(), &4242);
    assert_eq!(db.port.source().category(), Category::Env);

    unsafe {
        std::env::remove_var(format!("{prefix}_HOST"));
        std::env::remove_var(format!("{prefix}_PORT"));
    }
}

#[cfg(feature = "env")]
#[test]
fn env_source_supports_nested_via_double_underscore() {
    let prefix = "PROVCFG_TEST_STEP11";
    unsafe {
        std::env::set_var(format!("{prefix}_NAME"), "my-app-env");
        std::env::set_var(format!("{prefix}_DATABASE__HOST"), "env.example");
    }

    let config = Config::new().add_env(prefix);

    let app = config.build::<AppConfigProv>().unwrap();

    assert_eq!(app.name.value(), "my-app-env");
    assert_eq!(app.name.source().category(), Category::Env);

    assert_eq!(app.database.host.value(), "env.example");
    assert_eq!(app.database.host.source().category(), Category::Env);

    assert_eq!(app.database.port.value(), &0);
    assert_eq!(app.database.port.source().category(), Category::Default);

    unsafe {
        std::env::remove_var(format!("{prefix}_NAME"));
        std::env::remove_var(format!("{prefix}_DATABASE__HOST"));
    }
}

#[cfg(feature = "env")]
#[test]
fn env_source_with_list_keys_splits_csv_for_named_paths() {
    use crate as provcfg;
    use crate::Configurable;

    #[derive(serde::Deserialize, Clone, Default, Configurable)]
    #[allow(dead_code)]
    struct Section {
        items: Vec<String>,
        other: String,
    }
    #[derive(serde::Deserialize, Clone, Default, Configurable)]
    #[allow(dead_code)]
    struct Top {
        #[configurable(nested)]
        section: Section,
    }

    let prefix = "PROVCFG_TEST_LIST_KEYS";
    unsafe {
        std::env::set_var(format!("{prefix}_SECTION__ITEMS"), "foo,bar,baz");
        std::env::set_var(format!("{prefix}_SECTION__OTHER"), "hello,world");
    }

    let config = Config::new().add_env_with_list_keys(prefix, ["section.items"]);
    let cfg = config.build::<TopProv>().unwrap();

    assert_eq!(
        cfg.section.items.value(),
        &vec!["foo".to_string(), "bar".to_string(), "baz".to_string()]
    );
    // `other` is not a list key, so the comma stays inside the string.
    assert_eq!(cfg.section.other.value(), "hello,world");

    unsafe {
        std::env::remove_var(format!("{prefix}_SECTION__ITEMS"));
        std::env::remove_var(format!("{prefix}_SECTION__OTHER"));
    }
}

#[cfg(feature = "cli")]
#[test]
fn cli_source_supplies_prebuilt_partial() {
    let cli = DatabaseConfigPartial {
        host: Some("cli.example".to_string()),
        port: None,
    };

    let config = Config::new().add_cli(cli);
    let db = config.build::<DatabaseConfigProv>().unwrap();

    assert_eq!(db.host.value(), "cli.example");
    assert_eq!(db.host.source().category(), Category::Cli);

    assert_eq!(db.port.value(), &0);
    assert_eq!(db.port.source().category(), Category::Default);
}

#[cfg(feature = "toml")]
#[test]
fn toml_source_provides_values_and_file_category() {
    let toml = r#"
host = "db.example"
port = 9000
"#;

    let config = Config::new().add_toml_str("app.toml", toml);

    let db = config.build::<DatabaseConfigProv>().unwrap();

    assert_eq!(db.host.value(), "db.example");
    assert_eq!(db.host.source().name(), "app.toml");
    assert_eq!(db.host.source().category(), Category::File);
    assert_eq!(db.port.value(), &9000);
    assert_eq!(db.port.source().category(), Category::File);
}

#[cfg(feature = "json")]
#[test]
fn add_json_file_reads_disk_and_tags_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.json");
    std::fs::write(&path, r#"{ "host": "from-file", "port": 8080 }"#).unwrap();

    let cfg = Config::new()
        .add_json_file(&path)
        .unwrap()
        .build::<DatabaseConfigProv>()
        .unwrap();

    assert_eq!(cfg.host.value(), "from-file");
    assert_eq!(cfg.port.value(), &8080);
    assert_eq!(cfg.host.source().category(), Category::File);
}

#[cfg(feature = "json")]
#[test]
fn add_json_file_missing_path_returns_io_error() {
    let err = match Config::new().add_json_file("/provcfg/no/such/file.json") {
        Ok(_) => panic!("expected an Io error for a missing path"),
        Err(e) => e,
    };
    assert!(matches!(err, Error::Io { .. }));
}

#[cfg(feature = "toml")]
#[test]
fn add_toml_file_reads_disk_and_tags_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.toml");
    std::fs::write(&path, "host = \"from-file\"\nport = 8080\n").unwrap();

    let cfg = Config::new()
        .add_toml_file(&path)
        .unwrap()
        .build::<DatabaseConfigProv>()
        .unwrap();

    assert_eq!(cfg.host.value(), "from-file");
    assert_eq!(cfg.port.value(), &8080);
    assert_eq!(cfg.host.source().category(), Category::File);
}

#[cfg(feature = "toml")]
#[test]
fn add_toml_file_missing_path_returns_io_error() {
    let err = match Config::new().add_toml_file("/provcfg/no/such/file.toml") {
        Ok(_) => panic!("expected an Io error for a missing path"),
        Err(e) => e,
    };
    assert!(matches!(err, Error::Io { .. }));
}

#[cfg(feature = "env")]
#[test]
fn env_parent_child_collision_resolves_deterministically_to_child() {
    // Setting both `<PREFIX>_DATABASE` (a scalar) and `<PREFIX>_DATABASE__HOST`
    // (a deeper key under the same parent) creates a parent/child collision.
    // After deterministic sort-by-key the scalar is processed first, then the
    // deeper key coerces it to an object and wins. The outcome is the same
    // regardless of the order returned by `std::env::vars()`.
    let prefix = "PROVCFG_COLLISION";
    unsafe {
        std::env::set_var(format!("{prefix}_DATABASE"), "ignored-scalar");
        std::env::set_var(format!("{prefix}_DATABASE__HOST"), "env.example");
    }

    let app = Config::new()
        .add_env(prefix)
        .build::<AppConfigProv>()
        .unwrap();

    assert_eq!(app.database.host.value(), "env.example");
    assert_eq!(app.database.host.source().category(), Category::Env);

    unsafe {
        std::env::remove_var(format!("{prefix}_DATABASE"));
        std::env::remove_var(format!("{prefix}_DATABASE__HOST"));
    }
}
