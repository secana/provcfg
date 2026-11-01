//! SQLite and PostgreSQL config sources for [`provcfg`], built on
//! [SeaORM](https://www.sea-ql.org/SeaORM/).
//!
//! Settings live in one key/value table, `provcfg_settings`, with one row per
//! leaf: a `namespace` (so several services can share the table), a dotted
//! `key` such as `database.port`, and a text `value`. [`load`] reads one
//! namespace into a [`KeyValueSource`] that layers like any other source and
//! reports [`provcfg::Category::Database`].
//!
//! # Setup
//!
//! - Enable the `sqlite` and/or `postgres` feature.
//! - This crate selects no async runtime or TLS backend. Enable one on your own
//!   `sea-orm` dependency, for example `runtime-tokio-rustls`. Without one,
//!   sqlx panics when the connection pool is created.
//! - Add [`migration::migrations`] to your migrator to create the table.
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "sqlite")]
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! use provcfg::{Category, Config, Configurable};
//! use sea_orm::{ConnectionTrait, Database};
//! use sea_orm_migration::SchemaManager;
//!
//! #[derive(Configurable, serde::Deserialize, Clone, Default)]
//! struct Settings {
//!     port: u16,
//! }
//!
//! let db = Database::connect("sqlite::memory:").await?;
//! for migration in provcfg_sea_orm::migration::migrations() {
//!     migration.up(&SchemaManager::new(&db)).await?;
//! }
//! db.execute_unprepared(
//!     "INSERT INTO provcfg_settings (namespace, key, value) VALUES ('svc', 'port', '8080')",
//! )
//! .await?;
//!
//! let settings = Config::new()
//!     .add_source(provcfg_sea_orm::load(&db, "svc").await?)
//!     .build::<SettingsProv>()
//!     .unwrap();
//!
//! assert_eq!(settings.port.value(), &8080);
//! assert_eq!(settings.port.source().category(), Category::Database);
//! # Ok::<(), sea_orm::DbErr>(())
//! # }).unwrap();
//! ```

use provcfg::sources::KeyValueSource;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QuerySelect};

pub mod entity;
pub mod migration;

use entity::{revision, setting};

/// PostgreSQL channel the triggers notify on, with the namespace as payload.
pub const NOTIFY_CHANNEL: &str = "provcfg_settings";

/// Read every row of `namespace` into a [`KeyValueSource`] named
/// `database:<namespace>`. Errors name the source, so the namespace shows up
/// in a failed [`provcfg::Config::build`].
///
/// Accepts any SeaORM connection, including a transaction.
///
/// # Errors
///
/// Returns the [`DbErr`] of the query, for example when the table does not
/// exist.
pub async fn load<C: ConnectionTrait>(db: &C, namespace: &str) -> Result<KeyValueSource, DbErr> {
    let rows = load_rows(db, namespace).await?;
    Ok(KeyValueSource::new(source_name(namespace), rows))
}

/// The change counter of `namespace`: bumped by a trigger on every insert,
/// update or delete of one of its rows, `0` if it never changed.
///
/// # Errors
///
/// Returns the [`DbErr`] of the query, for example when the
/// [`TrackRevisions`](migration::TrackRevisions) migration has not run.
pub async fn revision<C: ConnectionTrait>(db: &C, namespace: &str) -> Result<i64, DbErr> {
    let current = revision::Entity::find_by_id(namespace.to_string())
        .one(db)
        .await?;
    Ok(current.map_or(0, |row| row.revision))
}

async fn load_rows<C: ConnectionTrait>(
    db: &C,
    namespace: &str,
) -> Result<Vec<(String, String)>, DbErr> {
    setting::Entity::find()
        .select_only()
        .columns([setting::Column::Key, setting::Column::Value])
        .filter(setting::Column::Namespace.eq(namespace))
        .into_tuple()
        .all(db)
        .await
}

fn source_name(namespace: &str) -> String {
    format!("database:{namespace}")
}
