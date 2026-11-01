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

use sea_orm::DeriveIden;
use sea_orm_migration::prelude::*;

/// Every migration this crate needs, oldest first.
#[must_use]
pub fn migrations() -> Vec<Box<dyn MigrationTrait>> {
    vec![Box::new(CreateSettingsTable)]
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

#[derive(DeriveIden)]
enum ProvcfgSettings {
    Table,
    Namespace,
    Key,
    Value,
}
