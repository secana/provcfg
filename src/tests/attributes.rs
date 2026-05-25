use super::fixtures::*;
use crate::*;

#[cfg(feature = "json")]
#[test]
fn skip_attribute_excludes_field_from_config() {
    let config = Config::new().add_json_str("k.json", r#"{"host": "db.example"}"#);
    let cfg = config.build::<WithSkippedProv>().unwrap();

    assert_eq!(cfg.host.value(), "db.example");

    let map = cfg.sources_map();
    assert_eq!(map.get("host"), Some(&Category::File));
    assert!(
        !map.contains_key("runtime_state"),
        "skipped field must not appear in sources_map"
    );
}

#[cfg(feature = "env")]
#[test]
fn rename_attribute_changes_env_lookup_name() {
    let prefix = "PROVCFG_TEST_STEP13";
    unsafe {
        std::env::set_var(format!("{prefix}_HOSTNAME"), "x");
    }

    let config = Config::new().add_env(prefix);
    let cfg = config.build::<RenamedProv>().unwrap();

    assert_eq!(cfg.host.value(), "x");
    assert_eq!(cfg.host.source().category(), Category::Env);

    unsafe {
        std::env::remove_var(format!("{prefix}_HOSTNAME"));
    }
}

#[cfg(feature = "env")]
#[test]
fn env_list_attribute_accepts_comma_string_from_env() {
    let prefix = "PROVCFG_TEST_ENV_LIST";
    unsafe {
        std::env::set_var(format!("{prefix}_ITEMS"), "foo,bar,baz");
    }

    let config = Config::new().add_env(prefix);
    let cfg = config.build::<WithListProv>().unwrap();

    assert_eq!(
        cfg.items.value(),
        &vec!["foo".to_string(), "bar".to_string(), "baz".to_string()]
    );
    assert_eq!(cfg.items.source().category(), Category::Env);

    unsafe {
        std::env::remove_var(format!("{prefix}_ITEMS"));
    }
}

#[cfg(feature = "toml")]
#[test]
fn env_list_attribute_still_accepts_toml_array() {
    let config = Config::new().add_toml_str("k.toml", r#"items = ["foo", "bar"]"#);
    let cfg = config.build::<WithListProv>().unwrap();
    assert_eq!(
        cfg.items.value(),
        &vec!["foo".to_string(), "bar".to_string()]
    );
}

#[test]
fn secret_attribute_marks_value_history() {
    let config = Config::new();
    let cfg = config.build::<WithSecretProv>().unwrap();

    assert!(
        !cfg.name.is_secret(),
        "non-secret field should not be marked"
    );
    assert!(
        cfg.password.is_secret(),
        "secret-attributed field should be marked"
    );
}
