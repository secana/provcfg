//! Migrations for the tables in [`entity`](crate::entity).
//!
//! Add them to your own `MigratorTrait` implementation, after your existing
//! migrations:
//!
//! ```ignore
//! fn migrations() -> Vec<Box<dyn MigrationTrait>> {
//!     let mut all = vec![Box::new(m20240101_000001_create_users::Migration) as _];
//!     all.extend(provcfg_sea_orm::migration::migrations());
//!     all
//! }
//! ```
//!
//! Later releases may append migrations; never reorder or remove them.

use sea_orm::{ConnectionTrait, DbBackend, DeriveIden};
use sea_orm_migration::prelude::*;

/// Every migration this crate needs, oldest first.
#[must_use]
pub fn migrations() -> Vec<Box<dyn MigrationTrait>> {
    vec![Box::new(CreateSettingsTable), Box::new(TrackRevisions)]
}

/// Creates the `provcfg_settings` table.
pub struct CreateSettingsTable;

impl MigrationName for CreateSettingsTable {
    fn name(&self) -> &str {
        "m20261101_000001_provcfg_create_settings"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateSettingsTable {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ProvcfgSettings::Table)
                    .col(
                        ColumnDef::new(ProvcfgSettings::Namespace)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ProvcfgSettings::Key).string().not_null())
                    .col(ColumnDef::new(ProvcfgSettings::Value).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(ProvcfgSettings::Namespace)
                            .col(ProvcfgSettings::Key),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ProvcfgSettings::Table).to_owned())
            .await
    }
}

/// Creates the `provcfg_revisions` table and the triggers that bump a
/// namespace's revision on every insert, update or delete in
/// `provcfg_settings`. On PostgreSQL the trigger also sends a notification on
/// the [`NOTIFY_CHANNEL`](crate::NOTIFY_CHANNEL) channel with the namespace as
/// payload. `TRUNCATE` does not fire the triggers.
///
/// Supports SQLite and PostgreSQL; other backends return an error.
pub struct TrackRevisions;

impl MigrationName for TrackRevisions {
    fn name(&self) -> &str {
        "m20261101_000002_provcfg_track_revisions"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for TrackRevisions {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let triggers = match manager.get_database_backend() {
            DbBackend::Sqlite => SQLITE_TRIGGERS,
            DbBackend::Postgres => POSTGRES_TRIGGERS,
            other => return Err(unsupported(other)),
        };
        manager
            .create_table(
                Table::create()
                    .table(ProvcfgRevisions::Table)
                    .col(
                        ColumnDef::new(ProvcfgRevisions::Namespace)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ProvcfgRevisions::Revision)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(triggers)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let drop_triggers = match manager.get_database_backend() {
            DbBackend::Sqlite => SQLITE_DROP_TRIGGERS,
            DbBackend::Postgres => POSTGRES_DROP_TRIGGERS,
            other => return Err(unsupported(other)),
        };
        manager
            .get_connection()
            .execute_unprepared(drop_triggers)
            .await?;
        manager
            .drop_table(Table::drop().table(ProvcfgRevisions::Table).to_owned())
            .await
    }
}

fn unsupported(backend: DbBackend) -> DbErr {
    DbErr::Migration(format!(
        "provcfg-sea-orm supports SQLite and PostgreSQL, not {backend:?}"
    ))
}

const SQLITE_TRIGGERS: &str = "
CREATE TRIGGER provcfg_settings_insert AFTER INSERT ON provcfg_settings BEGIN
    INSERT INTO provcfg_revisions (namespace, revision) VALUES (NEW.namespace, 1)
        ON CONFLICT (namespace) DO UPDATE SET revision = revision + 1;
END;
CREATE TRIGGER provcfg_settings_update AFTER UPDATE ON provcfg_settings BEGIN
    INSERT INTO provcfg_revisions (namespace, revision) VALUES (NEW.namespace, 1)
        ON CONFLICT (namespace) DO UPDATE SET revision = revision + 1;
    INSERT INTO provcfg_revisions (namespace, revision) VALUES (OLD.namespace, 1)
        ON CONFLICT (namespace) DO UPDATE SET revision = revision + 1;
END;
CREATE TRIGGER provcfg_settings_delete AFTER DELETE ON provcfg_settings BEGIN
    INSERT INTO provcfg_revisions (namespace, revision) VALUES (OLD.namespace, 1)
        ON CONFLICT (namespace) DO UPDATE SET revision = revision + 1;
END;
";

const SQLITE_DROP_TRIGGERS: &str = "
DROP TRIGGER IF EXISTS provcfg_settings_insert;
DROP TRIGGER IF EXISTS provcfg_settings_update;
DROP TRIGGER IF EXISTS provcfg_settings_delete;
";

const POSTGRES_TRIGGERS: &str = "
CREATE FUNCTION provcfg_settings_changed() RETURNS trigger AS $$
DECLARE
    ns text;
BEGIN
    FOREACH ns IN ARRAY CASE TG_OP
        WHEN 'INSERT' THEN ARRAY[NEW.namespace]
        WHEN 'DELETE' THEN ARRAY[OLD.namespace]
        ELSE ARRAY[NEW.namespace, OLD.namespace]
    END LOOP
        INSERT INTO provcfg_revisions (namespace, revision) VALUES (ns, 1)
            ON CONFLICT (namespace)
            DO UPDATE SET revision = provcfg_revisions.revision + 1;
        PERFORM pg_notify('provcfg_settings', ns);
    END LOOP;
    RETURN NULL;
END
$$ LANGUAGE plpgsql;
CREATE TRIGGER provcfg_settings_changed
    AFTER INSERT OR UPDATE OR DELETE ON provcfg_settings
    FOR EACH ROW EXECUTE FUNCTION provcfg_settings_changed();
";

const POSTGRES_DROP_TRIGGERS: &str = "
DROP TRIGGER IF EXISTS provcfg_settings_changed ON provcfg_settings;
DROP FUNCTION IF EXISTS provcfg_settings_changed();
";

#[derive(DeriveIden)]
enum ProvcfgRevisions {
    Table,
    Namespace,
    Revision,
}

#[derive(DeriveIden)]
enum ProvcfgSettings {
    Table,
    Namespace,
    Key,
    Value,
}
