use std::fmt::Display;
use std::sync::{Arc, Mutex, PoisonError};

use provcfg::sources::KeyValueSource;
use provcfg::{Category, Config, DynamicSource, LiveConfig, Provenance};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    TransactionTrait,
};

use crate::entity::setting;
use crate::{Error, load_rows, revision, source_name};

/// What a successful [`LiveSettings::set`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    /// Dotted paths whose active value or source changed, as reported by
    /// [`LiveConfig::reload`].
    pub changed: Vec<String>,
    /// The category of a higher-priority source that still sets this path, so
    /// the written value is stored but not active. `None` when the database
    /// value is the active one.
    pub hidden_by: Option<Category>,
}

/// A live config whose database layer can be refreshed, written and watched.
///
/// The database layer is one namespace of the `provcfg_settings` table. You
/// decide where it sits among the other sources when creating it. Cloning is
/// cheap and yields another handle to the same config.
///
/// Values are stored as text and converted like environment variables. Fields
/// marked `#[configurable(secret)]` are stored in plain text as well.
pub struct LiveSettings<T> {
    inner: Arc<Inner<T>>,
}

struct Inner<T> {
    db: DatabaseConnection,
    namespace: String,
    rows: DynamicSource,
    live: LiveConfig<T>,
    /// Revision of the rows currently installed; older loads are dropped.
    installed: Mutex<i64>,
}

impl<T> Clone for LiveSettings<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Rows of one namespace with the revision read just before them.
struct Snapshot {
    revision: i64,
    rows: Vec<(String, String)>,
}

impl<T: Provenance + Send + Sync + 'static> LiveSettings<T> {
    /// Load `namespace` and build the config. `layers` receives the database
    /// layer as a [`DynamicSource`] and returns the full [`Config`], so the
    /// database can sit anywhere in the priority order:
    ///
    /// ```ignore
    /// let settings = LiveSettings::<AppProv>::new(db, "billing", |rows| {
    ///     Ok(Config::new()
    ///         .add_toml_file("app.toml")?
    ///         .add_source(rows)
    ///         .add_env("APP"))
    /// })
    /// .await?;
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Db`] if loading fails, [`Error::Config`] if `layers` fails or
    /// the config does not build.
    pub async fn new<F>(
        db: DatabaseConnection,
        namespace: impl Into<String>,
        layers: F,
    ) -> Result<Self, Error>
    where
        F: FnOnce(DynamicSource) -> Result<Config, provcfg::Error>,
    {
        let namespace = namespace.into();
        let snapshot = snapshot(&db, &namespace).await?;
        let rows = DynamicSource::new(
            source_name(&namespace),
            KeyValueSource::new(source_name(&namespace), snapshot.rows),
        );
        let live = LiveConfig::new(layers(rows.clone())?)?;
        Ok(Self {
            inner: Arc::new(Inner {
                db,
                namespace,
                rows,
                live,
                installed: Mutex::new(snapshot.revision),
            }),
        })
    }

    /// The current config. See [`LiveConfig::load`].
    pub fn load(&self) -> Arc<T> {
        self.inner.live.load()
    }

    /// The underlying live config, for example to register
    /// [`on_change`](LiveConfig::on_change) callbacks.
    pub fn live(&self) -> &LiveConfig<T> {
        &self.inner.live
    }

    /// The namespace this config reads and writes.
    pub fn namespace(&self) -> &str {
        &self.inner.namespace
    }

    #[cfg(feature = "watch")]
    pub(crate) fn db(&self) -> &DatabaseConnection {
        &self.inner.db
    }

    #[cfg(feature = "watch")]
    pub(crate) fn installed_revision(&self) -> i64 {
        *self.lock_installed()
    }

    /// Read the namespace again and apply it. Returns the changed paths.
    ///
    /// # Errors
    ///
    /// [`Error::Db`] if loading fails. [`Error::Config`] if the rows do not fit
    /// the config type; the previous config stays current.
    pub async fn refresh(&self) -> Result<Vec<String>, Error> {
        let snapshot = snapshot(&self.inner.db, &self.inner.namespace).await?;
        let source = self.validated(snapshot.rows)?;
        self.install(snapshot.revision, source)
    }

    /// Store `value` (as its `Display` text) at the dotted `path` and apply
    /// it. Other instances pick the change up through their watcher.
    ///
    /// Nothing is written if the path is unknown, the value does not convert
    /// to the field type, or the namespace would no longer build.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownPath`], [`Error::InvalidValue`], [`Error::Config`] if
    /// other rows of the namespace are invalid, or [`Error::Db`].
    pub async fn set(&self, path: &str, value: impl Display) -> Result<WriteOutcome, Error> {
        if !self.leaf_paths().iter().any(|leaf| leaf == path) {
            return Err(Error::UnknownPath(path.to_string()));
        }
        let value = value.to_string();
        self.check_value(path, &value)?;

        let row = setting::ActiveModel {
            namespace: Set(self.inner.namespace.clone()),
            key: Set(path.to_string()),
            value: Set(value),
        };
        let upsert = setting::Entity::insert(row).on_conflict(
            OnConflict::columns([setting::Column::Namespace, setting::Column::Key])
                .update_column(setting::Column::Value)
                .to_owned(),
        );
        let txn = self.inner.db.begin().await?;
        upsert.exec_without_returning(&txn).await?;
        let changed = self.commit_and_install(txn).await?;

        let hidden_by = match self.active_category(path) {
            Some(Category::Database) => None,
            other => other,
        };
        Ok(WriteOutcome { changed, hidden_by })
    }

    /// Delete the row at `path` and apply the change, so lower layers take
    /// over again. Any key can be removed, including ones the config type no
    /// longer has. Returns the changed paths.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] if the remaining rows of the namespace are invalid
    /// (nothing is deleted then), or [`Error::Db`].
    pub async fn unset(&self, path: &str) -> Result<Vec<String>, Error> {
        let txn = self.inner.db.begin().await?;
        setting::Entity::delete_many()
            .filter(setting::Column::Namespace.eq(&self.inner.namespace))
            .filter(setting::Column::Key.eq(path))
            .exec(&txn)
            .await?;
        self.commit_and_install(txn).await
    }

    /// Validate the namespace as it is inside `txn`, commit, then apply it.
    /// Dropping `txn` on an error rolls the write back.
    async fn commit_and_install(
        &self,
        txn: sea_orm::DatabaseTransaction,
    ) -> Result<Vec<String>, Error> {
        let snapshot = snapshot(&txn, &self.inner.namespace).await?;
        let source = self.validated(snapshot.rows)?;
        txn.commit().await?;
        self.install(snapshot.revision, source)
    }

    /// Build the database layer on its own. Each source is deserialized
    /// independently, so this is exactly the check the full build would run
    /// for it, without touching the live config.
    fn validated(&self, rows: Vec<(String, String)>) -> Result<KeyValueSource, Error> {
        let name = source_name(&self.inner.namespace);
        Config::new()
            .add_source(KeyValueSource::new(name.clone(), rows.clone()))
            .build::<T>()?;
        Ok(KeyValueSource::new(name, rows))
    }

    fn check_value(&self, path: &str, value: &str) -> Result<(), Error> {
        Config::new()
            .add_source(KeyValueSource::new(
                source_name(&self.inner.namespace),
                [(path, value)],
            ))
            .build::<T>()
            .map(|_| ())
            .map_err(|source| Error::InvalidValue {
                path: path.to_string(),
                source,
            })
    }

    /// Swap in `source` unless a newer revision is already installed.
    fn install(&self, revision: i64, source: KeyValueSource) -> Result<Vec<String>, Error> {
        let mut installed = self.lock_installed();
        if revision < *installed {
            return Ok(Vec::new());
        }
        self.inner.rows.replace(source);
        let changed = self.inner.live.reload()?;
        *installed = revision;
        Ok(changed)
    }

    fn lock_installed(&self) -> std::sync::MutexGuard<'_, i64> {
        self.inner
            .installed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn leaf_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        self.load()
            .walk_leaves("", &mut |path, _, _, _| paths.push(path.to_string()));
        paths
    }

    fn active_category(&self, path: &str) -> Option<Category> {
        self.load().sources_map().remove(path)
    }
}

async fn snapshot<C: ConnectionTrait>(db: &C, namespace: &str) -> Result<Snapshot, Error> {
    let revision = revision(db, namespace).await?;
    let rows = load_rows(db, namespace).await?;
    Ok(Snapshot { revision, rows })
}
