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
