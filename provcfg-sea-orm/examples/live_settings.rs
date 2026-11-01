//! A service whose config follows the database without a restart.
//!
//! ```text
//! # terminal 1: run the "service" and watch its config change
//! cargo run -p provcfg-sea-orm --example live_settings --features sqlite,postgres,watch -- \
//!     sqlite:///tmp/provcfg-demo.db?mode=rwc watch
//!
//! # terminal 2: write values through provcfg (validated) ...
//! cargo run -p provcfg-sea-orm --example live_settings --features sqlite,postgres,watch -- \
//!     sqlite:///tmp/provcfg-demo.db?mode=rwc set server.port 9090
//!
//! # ... or by hand; the watcher picks this up too
//! sqlite3 /tmp/provcfg-demo.db \
//!     "INSERT OR REPLACE INTO provcfg_settings VALUES ('demo', 'log_level', 'debug')"
//! ```
//!
//! Use a `postgres://user:password@host/db` URL instead to get instant updates
//! through `LISTEN/NOTIFY`. `DEMO_LOG_LEVEL=warn` pins `log_level` above the
//! database, so a write to it is stored but reported as hidden.

use std::time::Duration;

use provcfg::{Config, Configurable, Provenance};
use provcfg_sea_orm::{LiveSettings, WatchOptions};
use sea_orm::Database;
use sea_orm_migration::SchemaManager;

#[derive(Configurable, serde::Deserialize, Clone, Default)]
struct Server {
    host: String,
    port: u16,
}

#[derive(Configurable, serde::Deserialize, Clone, Default)]
struct Settings {
    log_level: String,
    #[configurable(nested)]
    server: Server,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, command, rest @ ..] = args.as_slice() else {
        return Err(
            "usage: live_settings <database-url> (watch | set <path> <value> | unset <path>)"
                .into(),
        );
    };

    let db = Database::connect(url.as_str()).await?;
    migrate(&db).await?;
    let settings = LiveSettings::<SettingsProv>::new(db, "demo", |rows| {
        Ok(Config::new().add_source(rows).add_env("DEMO"))
    })
    .await?;

    match (command.as_str(), rest) {
        ("watch", []) => watch(settings).await,
        ("set", [path, value]) => {
            let outcome = settings.set(path, value).await?;
            println!("changed: {:?}", outcome.changed);
            if let Some(category) = outcome.hidden_by {
                println!(
                    "stored, but the {} layer still overrides it",
                    category.as_str()
                );
            }
            Ok(())
        }
        ("unset", [path]) => {
            println!("changed: {:?}", settings.unset(path).await?);
            Ok(())
        }
        _ => Err(format!("unknown command {command:?} {rest:?}").into()),
    }
}

/// Run the crate's migrations once; a real service adds them to its own
/// `MigratorTrait` instead.
async fn migrate(db: &sea_orm::DatabaseConnection) -> Result<(), sea_orm::DbErr> {
    let manager = SchemaManager::new(db);
    if manager.has_table("provcfg_settings").await? {
        return Ok(());
    }
    for migration in provcfg_sea_orm::migration::migrations() {
        migration.up(&manager).await?;
    }
    Ok(())
}

async fn watch(settings: LiveSettings<SettingsProv>) -> Result<(), Box<dyn std::error::Error>> {
    print_config("initial config", &settings.load());
    settings.live().on_change(|change| {
        print_config(&format!("changed {:?}", change.changed), &change.new);
    });
    let options = WatchOptions::new()
        .poll_interval(Duration::from_secs(1))
        .on_error(|error| eprintln!("watch error: {error}"));
    println!("watching; press Ctrl-C to stop");
    settings.watch(options).await;
    Ok(())
}

fn print_config(title: &str, config: &SettingsProv) {
    println!("{title}:");
    config.walk_leaves("", &mut |path, value, category, _secret| {
        let value = serde_json::to_string(value).unwrap_or_default();
        println!("  {path} = {value} ({})", category.as_str());
    });
}
