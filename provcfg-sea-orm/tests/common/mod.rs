//! Backend-independent test cases. Each backend test file connects to its
//! database and runs every case through `backend_tests!`.

use std::time::Duration;

use provcfg::{Category, Config, Configurable, DynamicSource};
use provcfg_sea_orm::entity::setting;
use provcfg_sea_orm::migration::migrations;
use provcfg_sea_orm::{Error, LiveSettings, WatchOptions};
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Set};
use sea_orm_migration::SchemaManager;

#[derive(Configurable, serde::Deserialize, Clone, Default)]
pub struct Database {
    pub host: String,
    pub port: u16,
}

#[derive(Configurable, serde::Deserialize, Clone, Default)]
pub struct Settings {
    pub name: String,
    #[configurable(nested)]
    pub database: Database,
}

pub async fn migrate_up(db: &DatabaseConnection) {
    let manager = SchemaManager::new(db);
    for migration in migrations() {
        migration.up(&manager).await.unwrap();
    }
}

async fn migrate_down(db: &DatabaseConnection) {
    let manager = SchemaManager::new(db);
    for migration in migrations().iter().rev() {
        migration.down(&manager).await.unwrap();
    }
}

pub async fn insert(db: &DatabaseConnection, namespace: &str, key: &str, value: &str) {
    setting::ActiveModel {
        namespace: Set(namespace.into()),
        key: Set(key.into()),
        value: Set(value.into()),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn build(db: &DatabaseConnection, namespace: &str) -> Result<SettingsProv, provcfg::Error> {
    let source = provcfg_sea_orm::load(db, namespace).await.unwrap();
    Config::new().add_source(source).build::<SettingsProv>()
}

pub async fn empty_table_falls_back_to_defaults(db: DatabaseConnection) {
    migrate_up(&db).await;

    let settings = build(&db, "svc").await.unwrap();

    assert_eq!(settings.name.value(), "");
    assert_eq!(settings.name.source().category(), Category::Default);
}

pub async fn loads_typed_values_from_its_namespace_only(db: DatabaseConnection) {
    migrate_up(&db).await;
    insert(&db, "svc", "name", "billing").await;
    insert(&db, "svc", "database.port", "5432").await;
    insert(&db, "other", "name", "not-mine").await;

    let settings = build(&db, "svc").await.unwrap();

    assert_eq!(settings.name.value(), "billing");
    assert_eq!(settings.name.source().category(), Category::Database);
    assert_eq!(settings.database.port.value(), &5432);
    assert_eq!(
        settings.database.host.source().category(),
        Category::Default
    );
}

pub async fn source_name_includes_the_namespace(db: DatabaseConnection) {
    migrate_up(&db).await;
    insert(&db, "svc", "database.port", "not-a-port").await;

    let Err(err) = build(&db, "svc").await else {
        panic!("expected a deserialize error for a non-numeric port");
    };

    let msg = err.to_string();
    assert!(msg.contains("database:svc"), "got: {msg}");
}

pub async fn migration_down_drops_the_table(db: DatabaseConnection) {
    migrate_up(&db).await;
    migrate_down(&db).await;

    assert!(provcfg_sea_orm::load(&db, "svc").await.is_err());
}

/// Database layer below a JSON file that pins `name`, so writes to `name` are
/// hidden by a higher-priority source.
pub async fn live(db: &DatabaseConnection) -> LiveSettings<SettingsProv> {
    migrate_up(db).await;
    live_settings(db).await
}

async fn live_settings(db: &DatabaseConnection) -> LiveSettings<SettingsProv> {
    LiveSettings::new(db.clone(), "svc", |rows: DynamicSource| {
        Ok(Config::new()
            .add_source(rows)
            .add_json_str("override.json", r#"{ "name": "from-file" }"#))
    })
    .await
    .unwrap()
}

async fn stored(db: &DatabaseConnection, key: &str) -> Option<String> {
    setting::Entity::find_by_id(("svc".to_string(), key.to_string()))
        .one(db)
        .await
        .unwrap()
        .map(|row| row.value)
}

pub async fn live_settings_layer_the_database_with_other_sources(db: DatabaseConnection) {
    migrate_up(&db).await;
    insert(&db, "svc", "name", "from-db").await;
    insert(&db, "svc", "database.port", "5432").await;

    let settings = live_settings(&db).await.load();

    assert_eq!(settings.name.value(), "from-file");
    assert_eq!(settings.database.port.value(), &5432);
    assert_eq!(
        settings.database.port.source().category(),
        Category::Database
    );
}

pub async fn set_writes_the_row_and_applies_it_locally(db: DatabaseConnection) {
    let settings = live(&db).await;

    let outcome = settings.set("database.port", 6543).await.unwrap();

    assert_eq!(outcome.changed, ["database.port"]);
    assert_eq!(outcome.hidden_by, None);
    assert_eq!(settings.load().database.port.value(), &6543);
    assert_eq!(stored(&db, "database.port").await.as_deref(), Some("6543"));
}

pub async fn set_overwrites_an_existing_row(db: DatabaseConnection) {
    let settings = live(&db).await;
    settings.set("database.host", "a.example").await.unwrap();

    settings.set("database.host", "b.example").await.unwrap();

    assert_eq!(
        stored(&db, "database.host").await.as_deref(),
        Some("b.example")
    );
    assert_eq!(settings.load().database.host.value(), "b.example");
}

pub async fn set_rejects_an_unknown_path(db: DatabaseConnection) {
    let settings = live(&db).await;

    for path in ["database.missing", "database", ""] {
        let result = settings.set(path, "x").await;

        assert!(
            matches!(result, Err(Error::UnknownPath(ref p)) if p == path),
            "{path}"
        );
    }
    assert_eq!(stored(&db, "database").await, None);
}

pub async fn set_rejects_a_value_of_the_wrong_type(db: DatabaseConnection) {
    let settings = live(&db).await;

    let result = settings.set("database.port", "not-a-port").await;

    assert!(matches!(result, Err(Error::InvalidValue { ref path, .. }) if path == "database.port"));
    assert_eq!(stored(&db, "database.port").await, None);
    assert_eq!(settings.load().database.port.value(), &0);
}

pub async fn set_reports_a_value_hidden_by_a_higher_priority_source(db: DatabaseConnection) {
    let settings = live(&db).await;

    let outcome = settings.set("name", "from-db").await.unwrap();

    assert_eq!(outcome.hidden_by, Some(Category::File));
    assert!(outcome.changed.is_empty());
    assert_eq!(settings.load().name.value(), "from-file");
    assert_eq!(stored(&db, "name").await.as_deref(), Some("from-db"));
}

pub async fn unset_removes_the_row_and_falls_back(db: DatabaseConnection) {
    let settings = live(&db).await;
    settings.set("database.port", 6543).await.unwrap();

    let changed = settings.unset("database.port").await.unwrap();

    assert_eq!(changed, ["database.port"]);
    assert_eq!(stored(&db, "database.port").await, None);
    assert_eq!(
        settings.load().database.port.source().category(),
        Category::Default
    );
}

pub async fn unset_of_a_missing_key_changes_nothing(db: DatabaseConnection) {
    let settings = live(&db).await;

    assert!(settings.unset("stale.key").await.unwrap().is_empty());
}

pub async fn refresh_picks_up_external_changes(db: DatabaseConnection) {
    let settings = live(&db).await;
    insert(&db, "svc", "database.host", "external.example").await;

    let changed = settings.refresh().await.unwrap();

    assert_eq!(changed, ["database.host"]);
    assert_eq!(settings.load().database.host.value(), "external.example");
}

pub async fn refresh_with_invalid_external_data_keeps_the_config(db: DatabaseConnection) {
    let settings = live(&db).await;
    insert(&db, "svc", "database.port", "broken").await;

    assert!(matches!(settings.refresh().await, Err(Error::Config(_))));
    assert_eq!(settings.load().database.port.value(), &0);
}

pub async fn every_write_bumps_the_namespace_revision(db: DatabaseConnection) {
    migrate_up(&db).await;
    assert_eq!(provcfg_sea_orm::revision(&db, "svc").await.unwrap(), 0);

    insert(&db, "svc", "name", "a").await;
    let after_insert = provcfg_sea_orm::revision(&db, "svc").await.unwrap();
    db.execute_unprepared("UPDATE provcfg_settings SET value = 'b' WHERE key = 'name'")
        .await
        .unwrap();
    let after_update = provcfg_sea_orm::revision(&db, "svc").await.unwrap();
    db.execute_unprepared("DELETE FROM provcfg_settings")
        .await
        .unwrap();
    let after_delete = provcfg_sea_orm::revision(&db, "svc").await.unwrap();

    assert!(0 < after_insert && after_insert < after_update && after_update < after_delete);
    assert_eq!(provcfg_sea_orm::revision(&db, "other").await.unwrap(), 0);
}

/// On PostgreSQL the poll interval is far longer than the wait, so only
/// `LISTEN/NOTIFY` can deliver the change in time.
pub async fn watch_applies_external_changes(db: DatabaseConnection) {
    let settings = live(&db).await;
    let poll = match db.get_database_backend() {
        DbBackend::Postgres => Duration::from_secs(600),
        _ => Duration::from_millis(20),
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    settings.live().on_change(move |change| {
        let _ = tx.send(change.changed.clone());
    });
    let watcher = tokio::spawn(settings.watch(WatchOptions::new().poll_interval(poll)));
    // Give the watcher time to subscribe before the external write.
    tokio::time::sleep(Duration::from_millis(200)).await;

    insert(&db, "svc", "database.host", "pushed.example").await;
    let changed = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("watcher did not apply the external change in time")
        .unwrap();

    assert_eq!(changed, ["database.host"]);
    assert_eq!(settings.load().database.host.value(), "pushed.example");
    watcher.abort();
}

/// Generate one `#[tokio::test]` per shared case. `$connect` takes the test
/// name (backends may use it to isolate tests) and returns a connection, or
/// `None` to skip.
#[macro_export]
macro_rules! backend_tests {
    ($connect:path) => {
        $crate::backend_tests!(
            $connect;
            empty_table_falls_back_to_defaults,
            loads_typed_values_from_its_namespace_only,
            source_name_includes_the_namespace,
            migration_down_drops_the_table,
            live_settings_layer_the_database_with_other_sources,
            set_writes_the_row_and_applies_it_locally,
            set_overwrites_an_existing_row,
            set_rejects_an_unknown_path,
            set_rejects_a_value_of_the_wrong_type,
            set_reports_a_value_hidden_by_a_higher_priority_source,
            unset_removes_the_row_and_falls_back,
            unset_of_a_missing_key_changes_nothing,
            refresh_picks_up_external_changes,
            refresh_with_invalid_external_data_keeps_the_config,
            every_write_bumps_the_namespace_revision,
            watch_applies_external_changes,
        );
    };
    ($connect:path; $($case:ident),* $(,)?) => {
        $(
            #[tokio::test(flavor = "multi_thread")]
            async fn $case() {
                let Some(db) = $connect(stringify!($case)).await else {
                    return;
                };
                common::$case(db).await;
            }
        )*
    };
}
