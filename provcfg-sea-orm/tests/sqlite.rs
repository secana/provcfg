#![cfg(all(feature = "sqlite", feature = "watch"))]

mod common;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

/// Every test gets its own in-memory database. One pooled connection, since
/// each SQLite in-memory connection is a separate database.
async fn connect(_test: &str) -> Option<DatabaseConnection> {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).sqlx_logging(false);
    Some(Database::connect(options).await.unwrap())
}

backend_tests!(connect);
