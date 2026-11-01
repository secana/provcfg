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
    /// [`set`](crate::LiveSettings::set) was called with a path that is not a
    /// leaf of the config type.
    #[error("unknown config path {0:?}")]
    UnknownPath(String),
    /// [`set`](crate::LiveSettings::set) was called with a value that does not
    /// convert to the field type. Nothing was written.
    #[error("invalid value for {path:?}: {source}")]
    InvalidValue {
        path: String,
        #[source]
        source: provcfg::Error,
    },
}
