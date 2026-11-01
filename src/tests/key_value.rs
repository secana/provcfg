use super::fixtures::*;
use crate as provcfg;
use crate::sources::KeyValueSource;
use crate::*;

#[derive(serde::Deserialize, Clone, Default, Configurable)]
struct WithOptional {
    timeout: Option<u32>,
}

fn build<T: Provenance>(source: KeyValueSource) -> Result<T, Error> {
    Config::new().add_source(source).build::<T>()
}

#[test]
fn key_value_source_has_database_category() {
    let src = KeyValueSource::new("db", Vec::<(String, String)>::new());

    assert_eq!(src.category(), Category::Database);
}

#[test]
fn flat_keys_are_converted_to_the_field_type() {
    let db: DatabaseConfigProv = build(KeyValueSource::new(
        "db",
        [("host", "db.internal"), ("port", "5432")],
    ))
    .unwrap();

    assert_eq!(db.host.value(), "db.internal");
    assert_eq!(db.port.value(), &5432);
    assert_eq!(db.port.source().category(), Category::Database);
}

#[test]
fn dotted_keys_set_nested_fields() {
    let app: AppConfigProv = build(KeyValueSource::new(
        "db",
        [("name", "svc"), ("database.port", "6543")],
    ))
    .unwrap();

    assert_eq!(app.name.value(), "svc");
    assert_eq!(app.database.port.value(), &6543);
    assert_eq!(app.database.port.source().category(), Category::Database);
    assert_eq!(app.database.host.source().category(), Category::Default);
}

#[test]
fn keys_are_matched_verbatim() {
    let renamed: RenamedProv =
        build(KeyValueSource::new("db", [("HOSTNAME", "upper.example")])).unwrap();

    assert_eq!(renamed.host.value(), "upper.example");
}

#[test]
fn env_list_field_accepts_comma_separated_value() {
    let list: WithListProv = build(KeyValueSource::new("db", [("items", "a, b,c")])).unwrap();

    assert_eq!(list.items.value(), &["a", "b", "c"]);
}

#[test]
fn list_keys_split_comma_separated_values() {
    #[derive(serde::Deserialize, Clone, Default, Configurable)]
    struct Ports {
        ports: Vec<u16>,
    }

    let ports: PortsProv =
        build(KeyValueSource::new("db", [("ports", "80, 443")]).with_list_keys(["ports"])).unwrap();

    assert_eq!(ports.ports.value(), &[80, 443]);
}

#[test]
fn present_key_makes_option_some() {
    let opt: WithOptionalProv = build(KeyValueSource::new("db", [("timeout", "30")])).unwrap();

    assert_eq!(opt.timeout.value(), &Some(30));
}

#[test]
fn invalid_value_is_a_deserialize_error_naming_the_source() {
    let result = build::<DatabaseConfigProv>(KeyValueSource::new("settings-db", [("port", "abc")]));

    let Err(err) = result else {
        panic!("expected a deserialize error for a non-numeric port");
    };
    let msg = err.to_string();
    assert!(msg.contains("settings-db"), "got: {msg}");
    assert!(msg.contains("abc"), "got: {msg}");
}

#[test]
fn parent_child_collision_resolves_to_child_regardless_of_row_order() {
    for rows in [
        [("database", "flat"), ("database.port", "1")],
        [("database.port", "1"), ("database", "flat")],
    ] {
        let app: AppConfigProv = build(KeyValueSource::new("db", rows)).unwrap();

        assert_eq!(app.database.port.value(), &1);
    }
}

#[cfg(feature = "json")]
mod json_values {
    use std::collections::HashMap;

    use super::*;

    #[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
    struct Host {
        name: String,
        port: u16,
    }

    #[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum Transport {
        Tcp { port: u16 },
        Stdio,
    }

    #[derive(serde::Deserialize, serde::Serialize, Clone, Default, Configurable)]
    struct Structured {
        hosts: Vec<Host>,
        aliases: HashMap<String, String>,
        primary: Option<Host>,
        transport: Option<Transport>,
        pair: Option<(u16, String)>,
        level: String,
        port: u16,
    }

    fn json_source<const N: usize>(entries: [(&str, &str); N]) -> KeyValueSource {
        KeyValueSource::new("db", entries).with_json_values()
    }

    #[test]
    fn lists_maps_and_structs_are_parsed_as_json() {
        let config: StructuredProv = build(json_source([
            (
                "hosts",
                r#"[{"name": "a", "port": 1}, {"name": "b", "port": 2}]"#,
            ),
            ("aliases", r#"{"x": "y"}"#),
            ("primary", r#"{"name": "p", "port": 3}"#),
            ("pair", r#"[7, "seven"]"#),
        ]))
        .unwrap();

        assert_eq!(
            config.hosts.value(),
            &[
                Host {
                    name: "a".into(),
                    port: 1
                },
                Host {
                    name: "b".into(),
                    port: 2
                },
            ]
        );
        assert_eq!(config.aliases.value()["x"], "y");
        assert_eq!(config.primary.value().as_ref().unwrap().name, "p");
        assert_eq!(config.pair.value(), &Some((7, "seven".to_string())));
        assert_eq!(config.hosts.source().category(), Category::Database);
    }

    #[test]
    fn scalars_and_unit_variants_stay_plain_text() {
        let config: StructuredProv = build(json_source([
            ("level", "debug"),
            ("port", "8080"),
            ("transport", "stdio"),
        ]))
        .unwrap();

        assert_eq!(config.level.value(), "debug");
        assert_eq!(config.port.value(), &8080);
        assert_eq!(config.transport.value(), &Some(Transport::Stdio));
    }

    #[test]
    fn enum_variants_with_data_are_json_objects() {
        let config: StructuredProv =
            build(json_source([("transport", r#"{"tcp": {"port": 9000}}"#)])).unwrap();

        assert_eq!(
            config.transport.value(),
            &Some(Transport::Tcp { port: 9000 })
        );
    }

    #[test]
    fn invalid_json_names_the_value() {
        let Err(err) = build::<StructuredProv>(json_source([("hosts", "[{")])) else {
            panic!("expected invalid JSON to fail");
        };

        assert!(err.to_string().contains("invalid JSON value"), "got: {err}");
    }

    #[test]
    fn json_of_the_wrong_shape_is_rejected() {
        assert!(build::<StructuredProv>(json_source([("hosts", r#"{"name": "a"}"#)])).is_err());
        assert!(build::<StructuredProv>(json_source([("hosts", r#"[{"name": "a"}]"#)])).is_err());
    }

    #[test]
    fn without_json_values_structured_text_is_an_error() {
        let source = KeyValueSource::new("db", [("hosts", r#"[{"name": "a", "port": 1}]"#)]);

        assert!(build::<StructuredProv>(source).is_err());
    }

    #[test]
    fn list_keys_are_still_split() {
        #[derive(serde::Deserialize, Clone, Default, Configurable)]
        struct Tags {
            tags: Vec<String>,
        }
        let source = json_source([("tags", "a, b")]).with_list_keys(["tags"]);

        let config: TagsProv = build(source).unwrap();

        assert_eq!(config.tags.value(), &["a", "b"]);
    }
}
