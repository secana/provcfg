use sea_orm::DbErr;

/// Errors of [`LiveSettings`](crate::LiveSettings).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A query or transaction failed.
    #[error("database error: {0}")]
    Db(#[from] DbErr),
    /// The settings in the database, or another layer, do not fit the config
    /// type. The previous config stays current.
    #[error(transparent)]
    Config(#[from] provcfg::Error),
    /// A write named a path that is not a leaf of the config type.
    #[error("unknown config path {0:?}")]
    UnknownPath(String),
    /// A write was given a value that does not convert to the field type.
    /// Nothing was written.
    #[error("invalid value for {path:?}: {source}")]
    InvalidValue {
        path: String,
        #[source]
        source: provcfg::Error,
    },
    /// A write's value could not be serialized. Nothing was written.
    #[error("cannot serialize the value for {path:?}: {source}")]
    Serialize {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    /// The key may not be set in the database (see
    /// [`LiveSettingsBuilder::writable`](crate::LiveSettingsBuilder::writable)
    /// and [`read_only`](crate::LiveSettingsBuilder::read_only)): either a
    /// write for it, or a row with it that was stored by other means. Nothing
    /// was written or applied; [`unset`](crate::LiveSettings::unset) removes
    /// such a row.
    #[error("config key {0:?} may not be set in the database")]
    NotWritable(String),
    /// A write expected another namespace revision, so the settings changed
    /// since they were read. Nothing was written.
    #[error("the settings changed: expected revision {expected}, found {actual}")]
    Conflict { expected: i64, actual: i64 },
}
