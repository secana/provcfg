use super::fixtures::*;
use crate::*;

#[test]
fn category_serializes_to_lowercase_string() {
    assert_eq!(
        serde_json::to_string(&Category::Default).unwrap(),
        "\"default\""
    );
    assert_eq!(serde_json::to_string(&Category::File).unwrap(), "\"file\"");
    assert_eq!(serde_json::to_string(&Category::Env).unwrap(), "\"env\"");
    assert_eq!(serde_json::to_string(&Category::Cli).unwrap(), "\"cli\"");
}

#[test]
fn walk_leaves_flags_secret_fields() {
    let config = Config::new();
    let cfg = config.build::<WithSecretProv>().unwrap();

    let mut secret_flags: Vec<(String, bool)> = Vec::new();
    cfg.walk_leaves("", &mut |path, _value, _cat, secret| {
        secret_flags.push((path.to_string(), secret));
    });
    secret_flags.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(
        secret_flags,
        vec![("name".to_string(), false), ("password".to_string(), true)]
    );
}

#[cfg(all(feature = "toml", feature = "json"))]
#[test]
fn walk_leaves_yields_path_value_category_per_leaf() {
    use crate::sources::TomlStr;

    let toml = r#"
name = "my-app"
[database]
host = "db.example"
"#;

    let config = Config::new().add_source(TomlStr::new("k.toml", toml));
    let app = config.build::<AppConfigProv>().unwrap();

    let mut leaves: Vec<(String, serde_json::Value, Category, bool)> = Vec::new();
    app.walk_leaves("", &mut |path, value, cat, secret| {
        leaves.push((
            path.to_string(),
            serde_json::to_value(value).unwrap(),
            cat,
            secret,
        ));
    });

    leaves.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(
        leaves.len(),
        3,
        "expected 3 leaves (name, database.host, database.port)"
    );
    assert_eq!(leaves[0].0, "database.host");
    assert_eq!(leaves[0].1, serde_json::json!("db.example"));
    assert_eq!(leaves[0].2, Category::File);
    assert!(!leaves[0].3);
    assert_eq!(leaves[1].0, "database.port");
    assert_eq!(leaves[1].1, serde_json::json!(0));
    assert_eq!(leaves[1].2, Category::Default);
    assert!(!leaves[1].3);
    assert_eq!(leaves[2].0, "name");
    assert_eq!(leaves[2].1, serde_json::json!("my-app"));
    assert_eq!(leaves[2].2, Category::File);
    assert!(!leaves[2].3);
}

#[cfg(feature = "toml")]
#[test]
fn prov_serializes_active_values_through_serde() {
    use crate::sources::TomlStr;

    let toml = r#"
name = "my-app"
[database]
host = "db.example"
"#;

    let config = Config::new().add_source(TomlStr::new("k.toml", toml));
    let app = config.build::<AppConfigProv>().unwrap();

    let json = serde_json::to_value(&app).expect("Prov must Serialize");
    assert_eq!(json["name"], serde_json::json!("my-app"));
    assert_eq!(json["database"]["host"], serde_json::json!("db.example"));
    assert_eq!(json["database"]["port"], serde_json::json!(0));
}
