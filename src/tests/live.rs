#![cfg(feature = "live")]

use std::sync::{Arc, Mutex};

use super::fixtures::*;
use crate::sources::KeyValueSource;
use crate::*;

/// `(old port, new port, changed paths)` as seen by an `on_change` callback.
type SeenChange = (u16, u16, Vec<String>);

fn rows(port: &str) -> KeyValueSource {
    KeyValueSource::new("rows", [("database.port", port)])
}

fn live_with(dynamic: &DynamicSource) -> LiveConfig<AppConfigProv> {
    LiveConfig::new(Config::new().add_source(dynamic.clone())).unwrap()
}

#[test]
fn live_config_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LiveConfig<AppConfigProv>>();
    assert_send_sync::<DynamicSource>();
}

#[test]
fn dynamic_source_keeps_its_name_and_reports_the_current_category() {
    let dynamic = DynamicSource::new("settings-db", rows("1"));

    assert_eq!(dynamic.name(), "settings-db");
    assert_eq!(dynamic.category(), Category::Database);
}

#[test]
fn load_returns_the_initially_built_config() {
    let dynamic = DynamicSource::new("db", rows("1"));

    let live = live_with(&dynamic);

    assert_eq!(live.load().database.port.value(), &1);
}

#[test]
fn reload_applies_the_replaced_source_and_reports_changed_paths() {
    let dynamic = DynamicSource::new("db", rows("1"));
    let live = live_with(&dynamic);

    dynamic.replace(rows("2"));
    let changed = live.reload().unwrap();

    assert_eq!(changed, ["database.port"]);
    assert_eq!(live.load().database.port.value(), &2);
}

#[test]
fn reload_without_replacement_reports_nothing() {
    let dynamic = DynamicSource::new("db", rows("1"));
    let live = live_with(&dynamic);

    assert!(live.reload().unwrap().is_empty());
}

#[test]
fn snapshots_taken_before_a_reload_stay_unchanged() {
    let dynamic = DynamicSource::new("db", rows("1"));
    let live = live_with(&dynamic);
    let before = live.load();

    dynamic.replace(rows("2"));
    live.reload().unwrap();

    assert_eq!(before.database.port.value(), &1);
}

#[test]
fn failed_reload_keeps_the_previous_config() {
    let dynamic = DynamicSource::new("db", rows("1"));
    let live = live_with(&dynamic);
    let calls = Arc::new(Mutex::new(0));
    let counter = calls.clone();
    live.on_change(move |_| *counter.lock().unwrap() += 1);

    dynamic.replace(rows("not-a-port"));

    assert!(matches!(live.reload(), Err(Error::Deserialize { .. })));
    assert_eq!(live.load().database.port.value(), &1);
    assert_eq!(*calls.lock().unwrap(), 0);
}

#[test]
fn on_change_receives_old_new_and_changed_paths_only_on_change() {
    let dynamic = DynamicSource::new("db", rows("1"));
    let live = live_with(&dynamic);
    let seen: Arc<Mutex<Vec<SeenChange>>> = Arc::default();
    let sink = seen.clone();
    live.on_change(move |change: &Change<AppConfigProv>| {
        sink.lock().unwrap().push((
            *change.old.database.port.value(),
            *change.new.database.port.value(),
            change.changed.clone(),
        ));
    });

    live.reload().unwrap();
    dynamic.replace(rows("2"));
    live.reload().unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        [(1, 2, vec!["database.port".to_string()])]
    );
}

#[test]
fn change_hidden_by_a_higher_priority_source_is_not_reported_but_recorded() {
    let dynamic = DynamicSource::new("db", rows("1"));
    let config = Config::new()
        .add_source(dynamic.clone())
        .add_source(KeyValueSource::new("override", [("database.port", "9")]));
    let live: LiveConfig<AppConfigProv> = LiveConfig::new(config).unwrap();

    dynamic.replace(rows("2"));
    let changed = live.reload().unwrap();

    assert!(changed.is_empty());
    let current = live.load();
    assert_eq!(current.database.port.value(), &9);
    let history: Vec<u16> = current
        .database
        .port
        .history()
        .iter()
        .map(|v| v.value)
        .collect();
    assert_eq!(history, [0, 2, 9]);
}

#[test]
fn source_change_with_the_same_value_counts_as_a_change() {
    let dynamic = DynamicSource::new(
        "db",
        KeyValueSource::new("rows", Vec::<(String, String)>::new()),
    );
    let live = live_with(&dynamic);
    assert_eq!(
        live.load().database.port.source().category(),
        Category::Default
    );

    dynamic.replace(rows("0"));

    assert_eq!(live.reload().unwrap(), ["database.port"]);
    assert_eq!(
        live.load().database.port.source().category(),
        Category::Database
    );
}
