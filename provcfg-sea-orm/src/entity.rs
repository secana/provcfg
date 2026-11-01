//! SeaORM entities for the tables this crate manages.

/// One config leaf: the `provcfg_settings` table.
pub mod setting {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "provcfg_settings")]
    pub struct Model {
        /// Separates the settings of different services sharing the table.
        #[sea_orm(primary_key, auto_increment = false)]
        pub namespace: String,
        /// Dotted path of the leaf, for example `database.port`.
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        /// Raw text, converted to the field type when the config is built.
        #[sea_orm(column_type = "Text")]
        pub value: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
