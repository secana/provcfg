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

/// Field types that exercise how raw env strings are coerced into the target
/// type: `String` keeps the value verbatim, scalars are parsed from it.
#[cfg(feature = "env")]
mod env_typing {
    use crate as provcfg;
    use crate::{Category, Config, Configurable};

    #[derive(serde::Deserialize, Clone, Default, Configurable)]
    pub struct Inner {
        pub secret: String,
        pub retries: u16,
    }

    #[derive(serde::Deserialize, Clone, Default, Configurable)]
    pub struct Typed {
        pub text: String,
        pub port: u16,
        pub offset: i64,
        pub ratio: f64,
        pub enabled: bool,
        pub maybe_text: Option<String>,
        pub maybe_port: Option<u16>,
        #[configurable(nested)]
        pub inner: Inner,
        pub items: Vec<String>,
        pub numbers: Vec<u16>,
    }

    /// Set the given `<prefix>_<suffix>` vars, build `Typed` from env only,
    /// remove the vars again and return the build result.
    fn build_with(
        prefix: &str,
        vars: &[(&str, &str)],
        list_keys: &[&str],
    ) -> Result<TypedProv, crate::Error> {
        // SAFETY: every test passes a unique prefix; no other test reads or
        // writes these vars.
        unsafe {
            for (k, v) in vars {
                std::env::set_var(format!("{prefix}_{k}"), v);
            }
        }
        let result = Config::new()
            .add_env_with_list_keys(prefix, list_keys.iter().copied())
            .build::<TypedProv>();
        unsafe {
            for (k, _) in vars {
                std::env::remove_var(format!("{prefix}_{k}"));
            }
        }
        result
    }

    fn text_of(prefix: &str, value: &str) -> String {
        build_with(prefix, &[("TEXT", value)], &[])
            .unwrap()
            .text
            .value()
            .clone()
    }

    #[test]
    fn numeric_looking_value_stays_a_string() {
        assert_eq!(text_of("PROVCFG_TYPING_NUMERIC", "123456"), "123456");
        assert_eq!(text_of("PROVCFG_TYPING_FLOAT", "1.50"), "1.50");
        assert_eq!(text_of("PROVCFG_TYPING_LEADING_ZERO", "007"), "007");
    }

    #[test]
    fn bool_looking_value_stays_a_string() {
        assert_eq!(text_of("PROVCFG_TYPING_TRUE", "true"), "true");
        assert_eq!(text_of("PROVCFG_TYPING_FALSE", "false"), "false");
    }

    #[test]
    fn quoted_value_keeps_its_quotes() {
        assert_eq!(text_of("PROVCFG_TYPING_QUOTED", r#""abc""#), r#""abc""#);
    }

    #[test]
    fn null_value_is_the_literal_string_null() {
        let cfg = build_with("PROVCFG_TYPING_NULL", &[("TEXT", "null")], &[]).unwrap();
        assert_eq!(cfg.text.value(), "null");
        assert_eq!(cfg.text.source().category(), Category::Env);
    }

    #[test]
    fn json_looking_value_stays_a_string() {
        assert_eq!(text_of("PROVCFG_TYPING_ARRAY", "[1,2]"), "[1,2]");
        assert_eq!(text_of("PROVCFG_TYPING_OBJECT", r#"{"a":1}"#), r#"{"a":1}"#);
    }

    #[test]
    fn scalars_are_parsed_into_their_field_type() {
        let cfg = build_with(
            "PROVCFG_TYPING_SCALARS",
            &[
                ("PORT", "8000"),
                ("OFFSET", "-42"),
                ("RATIO", "0.25"),
                ("ENABLED", "true"),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(cfg.port.value(), &8000);
        assert_eq!(cfg.port.source().category(), Category::Env);
        assert_eq!(cfg.offset.value(), &-42);
        assert!((cfg.ratio.value() - 0.25).abs() < f64::EPSILON);
        assert!(cfg.enabled.value());
    }

    #[test]
    fn false_is_parsed_into_bool_field() {
        let cfg = build_with("PROVCFG_TYPING_BOOL_FALSE", &[("ENABLED", "false")], &[]).unwrap();
        assert!(!cfg.enabled.value());
        assert_eq!(cfg.enabled.source().category(), Category::Env);
    }

    #[test]
    fn unparsable_scalar_reports_the_value_and_type() {
        let err = match build_with("PROVCFG_TYPING_BAD_PORT", &[("PORT", "eighty")], &[]) {
            Ok(_) => panic!("expected a deserialize error"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("eighty"),
            "message should name the value: {msg}"
        );
        assert!(msg.contains("u16"), "message should name the type: {msg}");
    }

    #[test]
    fn out_of_range_number_is_rejected() {
        assert!(build_with("PROVCFG_TYPING_RANGE", &[("PORT", "70000")], &[]).is_err());
    }

    #[test]
    fn option_string_keeps_raw_value() {
        for (prefix, raw) in [
            ("PROVCFG_TYPING_OPT_STR", "hello"),
            ("PROVCFG_TYPING_OPT_DIGITS", "123"),
            ("PROVCFG_TYPING_OPT_NULL", "null"),
        ] {
            let cfg = build_with(prefix, &[("MAYBE_TEXT", raw)], &[]).unwrap();
            assert_eq!(cfg.maybe_text.value().as_deref(), Some(raw));
            assert_eq!(cfg.maybe_text.source().category(), Category::Env);
        }
    }

    #[test]
    fn option_number_is_parsed() {
        let cfg = build_with("PROVCFG_TYPING_OPT_NUM", &[("MAYBE_PORT", "8080")], &[]).unwrap();
        assert_eq!(cfg.maybe_port.value(), &Some(8080));
        assert_eq!(cfg.maybe_port.source().category(), Category::Env);
    }

    #[test]
    fn unset_option_falls_back_to_default() {
        let cfg = build_with("PROVCFG_TYPING_OPT_UNSET", &[("TEXT", "x")], &[]).unwrap();
        assert_eq!(cfg.maybe_port.value(), &None);
        assert_eq!(cfg.maybe_port.source().category(), Category::Default);
    }

    #[test]
    fn nested_keys_are_typed_by_their_field() {
        let cfg = build_with(
            "PROVCFG_TYPING_NESTED",
            &[("INNER__SECRET", "123456"), ("INNER__RETRIES", "3")],
            &[],
        )
        .unwrap();
        assert_eq!(cfg.inner.secret.value(), "123456");
        assert_eq!(cfg.inner.retries.value(), &3);
        assert_eq!(cfg.inner.secret.source().category(), Category::Env);
    }

    #[test]
    fn list_keys_split_and_type_each_element() {
        let cfg = build_with(
            "PROVCFG_TYPING_LISTS",
            &[("ITEMS", "1, true ,null"), ("NUMBERS", "80,443")],
            &["items", "numbers"],
        )
        .unwrap();
        assert_eq!(
            cfg.items.value(),
            &vec!["1".to_string(), "true".to_string(), "null".to_string()]
        );
        assert_eq!(cfg.numbers.value(), &vec![80, 443]);
    }

    #[test]
    fn empty_list_key_yields_empty_list() {
        let cfg = build_with("PROVCFG_TYPING_EMPTY_LIST", &[("ITEMS", "")], &["items"]).unwrap();
        assert!(cfg.items.value().is_empty());
        assert_eq!(cfg.items.source().category(), Category::Env);
    }
}
