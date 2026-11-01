use std::fmt::Display;
use std::sync::{Arc, Mutex, PoisonError};

use provcfg::sources::KeyValueSource;
use provcfg::{BoxError, Category, Config, DynamicSource, LiveConfig, Provenance};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, Set, TransactionTrait,
};
use serde::Serialize;

use crate::entity::{revision as revision_row, setting};
use crate::{Error, key_value_source, load_rows, revision, source_name};

/// What a successful write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    /// Dotted paths whose active value or source changed, as reported by
    /// [`LiveConfig::reload`].
    pub changed: Vec<String>,
    /// Written paths that a higher-priority source still sets, with that
    /// source's category: the value is stored but not active.
    pub hidden: Vec<(String, Category)>,
    /// The namespace revision after the write, for
    /// [`Batch::expect_revision`] of the next one.
    pub revision: i64,
}

/// A live config whose database layer can be refreshed, written and watched.
///
/// The database layer is one namespace of the `provcfg_settings` table. You
/// decide where it sits among the other sources when creating it. Cloning is
/// cheap and yields another handle to the same config.
///
/// Values are stored as text: scalars and strings as they are (`8080`,
/// `debug`), lists, maps, structs and enums with data as JSON. Fields marked
/// `#[configurable(secret)]` are stored in plain text as well.
pub struct LiveSettings<T> {
    inner: Arc<Inner<T>>,
}

struct Inner<T> {
    db: DatabaseConnection,
    namespace: String,
    keys: Keys,
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

/// Which keys the database may set. A key matches a listed path if it is
/// that path or below it (`server` matches `server.port`).
#[derive(Debug, Clone, Default)]
struct Keys {
    /// Empty means every key.
    only: Vec<String>,
    except: Vec<String>,
}

impl Keys {
    fn allows(&self, key: &str) -> bool {
        let matches = |path: &String| {
            key == path
                || key
                    .strip_prefix(path.as_str())
                    .is_some_and(|rest| rest.starts_with('.'))
        };
        (self.only.is_empty() || self.only.iter().any(matches)) && !self.except.iter().any(matches)
    }
}

type Validator<T> = Box<dyn Fn(&T) -> Result<(), BoxError> + Send + Sync>;

/// Options of a [`LiveSettings`], from [`LiveSettings::builder`].
pub struct LiveSettingsBuilder<T> {
    db: DatabaseConnection,
    namespace: String,
    keys: Keys,
    validator: Option<Validator<T>>,
}

impl<T: Provenance + Send + Sync + 'static> LiveSettingsBuilder<T> {
    /// Check every config built with database rows, see
    /// [`LiveConfig::with_validator`]. A write the validator rejects is not
    /// stored, and rows changed elsewhere that it rejects are not applied.
    #[must_use]
    pub fn validator<E: Into<BoxError>>(
        mut self,
        validator: impl Fn(&T) -> Result<(), E> + Send + Sync + 'static,
    ) -> Self {
        self.validator = Some(Box::new(move |config| {
            validator(config).map_err(Into::into)
        }));
        self
    }

    /// Let the database set only these paths and the keys below them, e.g.
    /// `["log_level", "server"]`. Without it, every path is writable.
    #[must_use]
    pub fn writable<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keys.only.extend(paths.into_iter().map(Into::into));
        self
    }

    /// Never let the database set these paths or the keys below them, e.g.
    /// settings needed to connect to the database or read only at startup.
    #[must_use]
    pub fn read_only<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keys.except.extend(paths.into_iter().map(Into::into));
        self
    }

    /// Load the namespace and build the config. `layers` receives the
    /// database layer as a [`DynamicSource`] and returns the full [`Config`],
    /// so the database can sit anywhere in the priority order:
    ///
    /// ```ignore
    /// let settings = LiveSettings::<AppProv>::builder(db, "billing")
    ///     .read_only(["database"])
    ///     .build(|rows| {
    ///         Ok(Config::new()
    ///             .add_toml_file("app.toml")?
    ///             .add_source(rows)
    ///             .add_env("APP"))
    ///     })
    ///     .await?;
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Db`] if loading fails, [`Error::NotWritable`] if a row has a
    /// key the database may not set, [`Error::Config`] if `layers` fails or
    /// the config does not build or validate.
    pub async fn build<F>(self, layers: F) -> Result<LiveSettings<T>, Error>
    where
        F: FnOnce(DynamicSource) -> Result<Config, provcfg::Error>,
    {
        let snapshot = snapshot(&self.db, &self.namespace).await?;
        let source = checked_source(&self.keys, &self.namespace, snapshot.rows)?;
        let rows = DynamicSource::new(source_name(&self.namespace), source);
        let config = layers(rows.clone())?;
        let live = match self.validator {
            Some(validator) => LiveConfig::with_validator(config, validator)?,
            None => LiveConfig::new(config)?,
        };
        Ok(LiveSettings {
            inner: Arc::new(Inner {
                db: self.db,
                namespace: self.namespace,
                keys: self.keys,
                rows,
                live,
                installed: Mutex::new(snapshot.revision),
            }),
        })
    }
}

/// Rows of one namespace with the revision read just before them.
struct Snapshot {
    revision: i64,
    rows: Vec<(String, String)>,
}

enum Op {
    Set { path: String, value: String },
    Unset { path: String },
}

/// Several writes applied together: validated as a whole, stored in one
/// transaction and applied with one reload. From [`LiveSettings::batch`].
#[must_use = "a batch does nothing until it is committed"]
pub struct Batch<'a, T> {
    settings: &'a LiveSettings<T>,
    ops: Vec<Op>,
    expected: Option<i64>,
    error: Option<Error>,
}

impl<T: Provenance + Send + Sync + 'static> Batch<'_, T> {
    /// Store `value` as its `Display` text at the dotted `path`. Lists, maps,
    /// structs and enums with data must be JSON text;
    /// [`set_value`](Self::set_value) converts them.
    pub fn set(mut self, path: &str, value: impl Display) -> Self {
        self.ops.push(Op::Set {
            path: path.to_string(),
            value: value.to_string(),
        });
        self
    }

    /// Store `value` at the dotted `path`: strings, numbers and booleans as
    /// plain text, everything else as JSON.
    pub fn set_value(mut self, path: &str, value: &impl Serialize) -> Self {
        match to_text(value) {
            Ok(value) => self.ops.push(Op::Set {
                path: path.to_string(),
                value,
            }),
            Err(source) => {
                self.error.get_or_insert(Error::Serialize {
                    path: path.to_string(),
                    source,
                });
            }
        }
        self
    }

    /// Delete the row at `path`, so lower layers take over again. Any key can
    /// be removed, including ones the config type no longer has or the
    /// database may not set.
    pub fn unset(mut self, path: &str) -> Self {
        self.ops.push(Op::Unset {
            path: path.to_string(),
        });
        self
    }

    /// Store nothing unless the namespace is still at `revision`, e.g. the
    /// [`LiveSettings::revision`] a form was loaded with, so a concurrent
    /// change is not overwritten silently.
    pub fn expect_revision(mut self, revision: i64) -> Self {
        self.expected = Some(revision);
        self
    }

    /// Validate, store and apply the writes. Other instances pick them up
    /// through their watcher. Nothing is stored if one of them fails.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownPath`], [`Error::NotWritable`], [`Error::InvalidValue`]
    /// or [`Error::Serialize`] for a write on its own; [`Error::Conflict`] if
    /// the revision moved; [`Error::Config`] if the namespace as a whole does
    /// not build or validate; [`Error::Db`].
    pub async fn commit(self) -> Result<WriteOutcome, Error> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let settings = self.settings;
        for op in &self.ops {
            if let Op::Set { path, value } = op {
                settings.check_set(path, value)?;
            }
        }

        let txn = settings.inner.db.begin().await?;
        let current = lock_revision(&txn, &settings.inner.namespace).await?;
        if let Some(expected) = self.expected
            && expected != current
        {
            return Err(Error::Conflict {
                expected,
                actual: current,
            });
        }
        for op in &self.ops {
            settings.apply(&txn, op).await?;
        }
        let snapshot = snapshot(&txn, &settings.inner.namespace).await?;
        settings.preview(snapshot.rows.clone())?;
        txn.commit().await?;
        let changed = settings.install(snapshot.revision, snapshot.rows)?;

        let sources = settings.load().sources_map();
        let mut hidden: Vec<(String, Category)> = Vec::new();
        for op in &self.ops {
            if let Op::Set { path, .. } = op
                && let Some(&category) = sources.get(path)
                && category != Category::Database
                && !hidden.iter().any(|(seen, _)| seen == path)
            {
                hidden.push((path.clone(), category));
            }
        }
        Ok(WriteOutcome {
            changed,
            hidden,
            revision: snapshot.revision,
        })
    }
}

impl<T: Provenance + Send + Sync + 'static> LiveSettings<T> {
    /// Load `namespace` and build the config with every key writable and no
    /// validator. Use [`builder`](Self::builder) for those options.
    ///
    /// # Errors
    ///
    /// See [`LiveSettingsBuilder::build`].
    pub async fn new<F>(
        db: DatabaseConnection,
        namespace: impl Into<String>,
        layers: F,
    ) -> Result<Self, Error>
    where
        F: FnOnce(DynamicSource) -> Result<Config, provcfg::Error>,
    {
        Self::builder(db, namespace).build(layers).await
    }

    /// Options for a live config of `namespace`: a validator and the keys the
    /// database may set.
    pub fn builder(db: DatabaseConnection, namespace: impl Into<String>) -> LiveSettingsBuilder<T> {
        LiveSettingsBuilder {
            db,
            namespace: namespace.into(),
            keys: Keys::default(),
            validator: None,
        }
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

    /// The revision of the rows in the current config, `0` if the namespace
    /// never changed. Pass it to [`Batch::expect_revision`].
    pub fn revision(&self) -> i64 {
        *self.lock_installed()
    }

    /// Whether the database may set `path`, see
    /// [`LiveSettingsBuilder::writable`] and [`LiveSettingsBuilder::read_only`].
    pub fn is_writable(&self, path: &str) -> bool {
        self.inner.keys.allows(path)
    }

    #[cfg(feature = "watch")]
    pub(crate) fn db(&self) -> &DatabaseConnection {
        &self.inner.db
    }

    /// Read the namespace again and apply it. Returns the changed paths.
    ///
    /// # Errors
    ///
    /// [`Error::Db`] if loading fails. [`Error::NotWritable`] or
    /// [`Error::Config`] if the rows do not fit; the previous config stays
    /// current.
    pub async fn refresh(&self) -> Result<Vec<String>, Error> {
        let snapshot = snapshot(&self.inner.db, &self.inner.namespace).await?;
        self.preview(snapshot.rows.clone())?;
        self.install(snapshot.revision, snapshot.rows)
    }

    /// Several writes applied together, see [`Batch`].
    pub fn batch(&self) -> Batch<'_, T> {
        Batch {
            settings: self,
            ops: Vec::new(),
            expected: None,
            error: None,
        }
    }

    /// Store `value` (as its `Display` text) at the dotted `path` and apply
    /// it. A batch with one [`Batch::set`].
    ///
    /// # Errors
    ///
    /// See [`Batch::commit`].
    pub async fn set(&self, path: &str, value: impl Display) -> Result<WriteOutcome, Error> {
        self.batch().set(path, value).commit().await
    }

    /// Store `value` at the dotted `path` and apply it, lists and structs as
    /// JSON. A batch with one [`Batch::set_value`].
    ///
    /// # Errors
    ///
    /// See [`Batch::commit`].
    pub async fn set_value(
        &self,
        path: &str,
        value: &impl Serialize,
    ) -> Result<WriteOutcome, Error> {
        self.batch().set_value(path, value).commit().await
    }

    /// Delete the row at `path` and apply the change. A batch with one
    /// [`Batch::unset`].
    ///
    /// # Errors
    ///
    /// See [`Batch::commit`].
    pub async fn unset(&self, path: &str) -> Result<WriteOutcome, Error> {
        self.batch().unset(path).commit().await
    }

    /// The checks of one write that need no database.
    fn check_set(&self, path: &str, value: &str) -> Result<(), Error> {
        if !self.leaf_paths().iter().any(|leaf| leaf == path) {
            return Err(Error::UnknownPath(path.to_string()));
        }
        if !self.is_writable(path) {
            return Err(Error::NotWritable(path.to_string()));
        }
        Config::new()
            .add_source(key_value_source(&self.inner.namespace, [(path, value)]))
            .build::<T>()
            .map(|_| ())
            .map_err(|source| Error::InvalidValue {
                path: path.to_string(),
                source,
            })
    }

    async fn apply(&self, txn: &DatabaseTransaction, op: &Op) -> Result<(), Error> {
        let namespace = &self.inner.namespace;
        match op {
            Op::Set { path, value } => {
                let row = setting::ActiveModel {
                    namespace: Set(namespace.clone()),
                    key: Set(path.clone()),
                    value: Set(value.clone()),
                };
                setting::Entity::insert(row)
                    .on_conflict(
                        OnConflict::columns([setting::Column::Namespace, setting::Column::Key])
                            .update_column(setting::Column::Value)
                            .to_owned(),
                    )
                    .exec_without_returning(txn)
                    .await?;
            }
            Op::Unset { path } => {
                setting::Entity::delete_many()
                    .filter(setting::Column::Namespace.eq(namespace))
                    .filter(setting::Column::Key.eq(path))
                    .exec(txn)
                    .await?;
            }
        }
        Ok(())
    }

    /// Build the full config with `rows` as the database layer, without
    /// applying it: checks the keys, the types and the validator.
    fn preview(&self, rows: Vec<(String, String)>) -> Result<(), Error> {
        let source = checked_source(&self.inner.keys, &self.inner.namespace, rows)?;
        self.inner.live.preview(&self.inner.rows, source)?;
        Ok(())
    }

    /// Swap in `rows` unless a newer revision is already installed.
    fn install(&self, revision: i64, rows: Vec<(String, String)>) -> Result<Vec<String>, Error> {
        let mut installed = self.lock_installed();
        if revision < *installed {
            return Ok(Vec::new());
        }
        self.inner
            .rows
            .replace(key_value_source(&self.inner.namespace, rows));
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
}

fn checked_source(
    keys: &Keys,
    namespace: &str,
    rows: Vec<(String, String)>,
) -> Result<KeyValueSource, Error> {
    if let Some((key, _)) = rows.iter().find(|(key, _)| !keys.allows(key)) {
        return Err(Error::NotWritable(key.clone()));
    }
    Ok(key_value_source(namespace, rows))
}

/// Strings, numbers and booleans as plain text, everything else as JSON.
fn to_text(value: &impl Serialize) -> Result<String, serde_json::Error> {
    Ok(match serde_json::to_value(value)? {
        serde_json::Value::String(text) => text,
        other @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_)) => other.to_string(),
        other => serde_json::to_string(&other)?,
    })
}

/// Serializes writers of a namespace and returns its revision. The no-op
/// upsert locks the revision row on PostgreSQL and takes the write lock on
/// SQLite, so the revision cannot move until the transaction ends.
async fn lock_revision(txn: &DatabaseTransaction, namespace: &str) -> Result<i64, Error> {
    let row = revision_row::ActiveModel {
        namespace: Set(namespace.to_string()),
        revision: Set(0),
    };
    revision_row::Entity::insert(row)
        .on_conflict(
            OnConflict::column(revision_row::Column::Namespace)
                .value(
                    revision_row::Column::Revision,
                    Expr::col((revision_row::Entity, revision_row::Column::Revision)),
                )
                .to_owned(),
        )
        .exec_without_returning(txn)
        .await?;
    Ok(revision(txn, namespace).await?)
}

async fn snapshot<C: ConnectionTrait>(db: &C, namespace: &str) -> Result<Snapshot, Error> {
    let revision = revision(db, namespace).await?;
    let rows = load_rows(db, namespace).await?;
    Ok(Snapshot { revision, rows })
}
