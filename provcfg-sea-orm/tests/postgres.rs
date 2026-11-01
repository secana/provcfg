#![cfg(feature = "postgres")]

//! Runs the shared cases against PostgreSQL at `PROVCFG_TEST_POSTGRES_URL`.
//! Without that variable the tests are skipped, except on CI (`CI` set),
//! where a missing database is an error.

mod common;

use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};

const URL_VAR: &str = "PROVCFG_TEST_POSTGRES_URL";

/// Every test gets a fresh schema named after it and a connection whose
/// `search_path` points there, so tests running in parallel never share
/// tables.
async fn connect(test: &str) -> Option<DatabaseConnection> {
    let Ok(url) = std::env::var(URL_VAR) else {
        assert!(
            std::env::var_os("CI").is_none(),
            "{URL_VAR} must be set on CI"
        );
        eprintln!("skipping {test}: {URL_VAR} is not set");
        return None;
    };
    let admin = Database::connect(&url).await.unwrap();
    admin
        .execute_unprepared(&format!(
            "DROP SCHEMA IF EXISTS {test} CASCADE; CREATE SCHEMA {test};"
        ))
        .await
        .unwrap();

    let mut options = ConnectOptions::new(url);
    options.set_schema_search_path(test).sqlx_logging(false);
    Some(Database::connect(options).await.unwrap())
}

backend_tests!(connect);
