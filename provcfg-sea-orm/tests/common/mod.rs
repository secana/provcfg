//! Backend-independent test cases. Each backend test file connects to its
//! database and runs every case through `backend_tests!`.

use provcfg::{Category, Config, Configurable};
use provcfg_sea_orm::entity::setting;
use provcfg_sea_orm::migration::migrations;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, Set};
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

async fn insert(db: &DatabaseConnection, namespace: &str, key: &str, value: &str) {
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
            every_write_bumps_the_namespace_revision,
        );
    };
    ($connect:path; $($case:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $case() {
                let Some(db) = $connect(stringify!($case)).await else {
                    return;
                };
                common::$case(db).await;
            }
        )*
    };
}
