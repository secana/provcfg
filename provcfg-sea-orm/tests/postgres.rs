#![cfg(all(feature = "postgres", feature = "watch"))]

//! Runs the shared cases against PostgreSQL at `PROVCFG_TEST_POSTGRES_URL`.
//! Without that variable the tests are skipped, except on CI (`CI` set),
//! where a missing database is an error.

mod common;

use std::time::Duration;

use provcfg_sea_orm::WatchOptions;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};

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

/// Kill every `LISTEN` connection (including those of tests running in
/// parallel, which must survive it too), then write. The poll interval is far
/// too long to help, so the change only arrives if the watcher reconnects.
#[tokio::test(flavor = "multi_thread")]
async fn watch_survives_a_lost_listener_connection() {
    let Some(db) = connect("watch_survives_a_lost_listener_connection").await else {
        return;
    };
    let settings = common::live(&db).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    settings.live().on_change(move |change| {
        let _ = tx.send(change.changed.clone());
    });
    let watcher =
        tokio::spawn(settings.watch(WatchOptions::new().poll_interval(Duration::from_secs(600))));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let killed = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(pg_terminate_backend(pid)) AS n FROM pg_stat_activity \
             WHERE query LIKE 'LISTEN%' AND pid <> pg_backend_pid()",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert!(killed > 0, "no listener connection was terminated");
    common::insert(&db, "svc", "database.host", "after-reconnect.example").await;

    let changed = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("watcher did not recover from the lost connection")
        .unwrap();
    assert_eq!(changed, ["database.host"]);
    watcher.abort();
}
