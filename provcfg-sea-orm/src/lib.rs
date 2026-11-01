//! SQLite and PostgreSQL config sources for [`provcfg`], built on
//! [SeaORM](https://www.sea-ql.org/SeaORM/), with live updates and write-back.
//!
//! Settings live in one key/value table, `provcfg_settings`, with one row per
//! leaf: a `namespace` (so several services can share the table), a dotted
//! `key` such as `database.port`, and a text `value`. Values are converted to
//! the field type like environment variables.
//!
//! - [`load`] reads one namespace into a [`KeyValueSource`] for a config that
//!   is built once.
//! - [`LiveSettings`] keeps a [`provcfg::LiveConfig`] in sync with the
//!   database: [`refresh`](LiveSettings::refresh) re-reads it,
//!   [`set`](LiveSettings::set) and [`unset`](LiveSettings::unset) write it
//!   (validated first), and [`watch`](LiveSettings::watch) applies changes made
//!   by other instances or by hand, without a restart.
//!
//! Files, environment variables and CLI arguments keep their values and their
//! priority; only the database layer changes at runtime.
//!
//! # Setup
//!
//! - Enable the `sqlite` and/or `postgres` feature, and `watch` for
//!   [`LiveSettings::watch`] (needs a tokio runtime).
//! - This crate selects no async runtime or TLS backend. Enable one on your own
//!   `sea-orm` dependency, for example `runtime-tokio-rustls`. Without one,
//!   sqlx panics when the connection pool is created.
//! - Add [`migration::migrations`] to your migrator. It creates the settings
//!   table and the triggers that track changes (a revision counter per
//!   namespace, plus `NOTIFY` on PostgreSQL).
//!
//! # Example
//!
//! ```
//! # #[cfg(all(feature = "sqlite", feature = "watch"))]
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! use std::time::Duration;
//!
//! use provcfg::{Category, Config, Configurable};
//! use provcfg_sea_orm::{LiveSettings, WatchOptions};
//! use sea_orm::{ConnectionTrait, Database};
//! use sea_orm_migration::SchemaManager;
//!
//! #[derive(Configurable, serde::Deserialize, Clone, Default)]
//! struct Settings {
//!     port: u16,
//!     log_level: String,
//! }
//!
//! let db = Database::connect("sqlite::memory:").await?;
//! for migration in provcfg_sea_orm::migration::migrations() {
//!     migration.up(&SchemaManager::new(&db)).await?;
//! }
//!
//! // The database layer sits between the defaults and the environment.
//! let settings = LiveSettings::<SettingsProv>::new(db.clone(), "billing", |rows| {
//!     Ok(Config::new().add_source(rows).add_env("BILLING"))
//! })
//! .await?;
//! settings
//!     .live()
//!     .on_change(|change| println!("config changed: {:?}", change.changed));
//!
//! // Keep the config in sync with changes made elsewhere.
//! tokio::spawn(settings.watch(WatchOptions::new().poll_interval(Duration::from_secs(2))));
//!
//! // Write a value: validated, stored, and applied right away.
//! let outcome = settings.set("log_level", "debug").await?;
//! assert_eq!(outcome.changed, ["log_level"]);
//! assert_eq!(settings.load().log_level.value(), "debug");
//!
//! // A change made directly in the database arrives through the watcher, or
//! // immediately with `refresh`.
//! db.execute_unprepared(
//!     "INSERT INTO provcfg_settings (namespace, key, value) VALUES ('billing', 'port', '8080')",
//! )
//! .await?;
//! settings.refresh().await?;
//! assert_eq!(settings.load().port.value(), &8080);
//! assert_eq!(settings.load().port.source().category(), Category::Database);
//! # Ok::<(), provcfg_sea_orm::Error>(())
//! # }).unwrap();
//! ```

use provcfg::sources::KeyValueSource;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QuerySelect};

pub mod entity;
mod error;
mod live_settings;
pub mod migration;
#[cfg(feature = "watch")]
mod watch;

use entity::{revision, setting};
pub use error::Error;
pub use live_settings::{LiveSettings, WriteOutcome};
#[cfg(feature = "watch")]
pub use watch::WatchOptions;

/// PostgreSQL channel the triggers notify on, with the namespace as payload.
pub const NOTIFY_CHANNEL: &str = "provcfg_settings";

/// Read every row of `namespace` into a [`KeyValueSource`] named
/// `database:<namespace>`. Errors name the source, so the namespace shows up
/// in a failed [`provcfg::Config::build`].
///
/// Accepts any SeaORM connection, including a transaction. For a config that
/// follows database changes, use [`LiveSettings`] instead.
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
