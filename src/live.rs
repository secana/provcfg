use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use arc_swap::ArcSwap;

use crate::{BoxError, Category, Config, Error, Provenance, Source, SourceArc};

thread_local! {
    /// Data that dynamic sources report instead of their own while
    /// [`LiveConfig::preview`] builds on this thread, keyed by the slot.
    static PREVIEWS: RefCell<Vec<(usize, SourceArc)>> = const { RefCell::new(Vec::new()) };
}

/// Removes the preview pushed last, also when the build panics.
struct PreviewGuard;

impl Drop for PreviewGuard {
    fn drop(&mut self) {
        PREVIEWS.with(|previews| previews.borrow_mut().pop());
    }
}

/// A [`Source`] whose data can be replaced at runtime.
///
/// Cloning yields another handle to the same slot: keep one clone, add the
/// other to a [`Config`], and call [`replace`](Self::replace) followed by
/// [`LiveConfig::reload`] when the underlying data changes. The name is fixed
/// at creation; the [`Category`] is that of the current inner source.
#[derive(Clone)]
pub struct DynamicSource {
    name: String,
    current: Arc<RwLock<SourceArc>>,
}

impl DynamicSource {
    /// Create a dynamic source. `name` is a human-readable label used in error
    /// messages.
    pub fn new<S: Source + Send + Sync + 'static>(name: impl Into<String>, source: S) -> Self {
        Self {
            name: name.into(),
            current: Arc::new(RwLock::new(Arc::new(source))),
        }
    }

    /// Swap in new data. Takes effect on the next build or reload.
    pub fn replace<S: Source + Send + Sync + 'static>(&self, source: S) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(source);
    }

    /// Identity of the shared slot, the same for every clone.
    fn slot(&self) -> usize {
        Arc::as_ptr(&self.current).addr()
    }

    fn inner(&self) -> SourceArc {
        let slot = self.slot();
        let preview = PREVIEWS.with(|previews| {
            let previews = previews.borrow();
            previews
                .iter()
                .rev()
                .find(|(key, _)| *key == slot)
                .map(|(_, source)| source.clone())
        });
        if let Some(source) = preview {
            return source;
        }
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Source for DynamicSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn category(&self) -> Category {
        self.inner().category()
    }

    fn deserialize(
        &self,
        seed: &mut dyn for<'de> FnMut(
            &mut dyn erased_serde::Deserializer<'de>,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        self.inner().deserialize(seed)
    }
}

/// What a successful [`LiveConfig::reload`] changed, as passed to
/// [`LiveConfig::on_change`] callbacks.
pub struct Change<T> {
    /// The config before the reload.
    pub old: Arc<T>,
    /// The config after the reload, now returned by [`LiveConfig::load`].
    pub new: Arc<T>,
    /// Dotted paths of the leaves whose value or source [`Category`] changed.
    pub changed: Vec<String>,
}

type Callback<T> = Arc<dyn Fn(&Change<T>) + Send + Sync>;
type Validator<T> = Box<dyn Fn(&T) -> Result<(), BoxError> + Send + Sync>;

/// A config that can be rebuilt at runtime without restarting the service.
///
/// Readers call [`load`](Self::load) for a consistent snapshot: a reload swaps
/// the whole config at once, so a snapshot never mixes old and new values.
/// [`reload`](Self::reload) builds the [`Config`] again. Only sources whose
/// data can change, such as [`DynamicSource`], produce different values;
/// files, environment variables and CLI arguments keep what they captured when
/// they were created, and keep their priority.
///
/// ```
/// use provcfg::sources::KeyValueSource;
/// use provcfg::{Config, Configurable, DynamicSource, LiveConfig};
///
/// # #[derive(Configurable, serde::Deserialize, Clone, Default)]
/// # struct Settings { port: u16 }
/// let rows = DynamicSource::new("settings-db", KeyValueSource::new("db", [("port", "80")]));
/// let live = LiveConfig::<SettingsProv>::new(Config::new().add_source(rows.clone())).unwrap();
///
/// live.on_change(|change| println!("changed: {:?}", change.changed));
///
/// rows.replace(KeyValueSource::new("db", [("port", "8080")]));
/// assert_eq!(live.reload().unwrap(), ["port"]);
/// assert_eq!(live.load().port.value(), &8080);
/// ```
pub struct LiveConfig<T> {
    config: Config,
    current: ArcSwap<T>,
    /// Serializes reloads so callbacks see changes in order.
    reload_lock: Mutex<()>,
    callbacks: Mutex<Vec<Callback<T>>>,
    validator: Option<Validator<T>>,
}

impl<T: Provenance> LiveConfig<T> {
    /// Build `config` once and keep it for later reloads.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Deserialize`] if a source's data does not fit `T`.
    pub fn new(config: Config) -> Result<Self, Error> {
        Self::create(config, None)
    }

    /// Like [`new`](Self::new), but every build must also pass `validator`:
    /// the first one, every [`reload`](Self::reload) and every
    /// [`preview`](Self::preview). Use it for rules across fields or layers
    /// that types cannot express, such as unique names or a value range. A
    /// rejected reload keeps the previous config.
    ///
    /// ```
    /// use provcfg::sources::KeyValueSource;
    /// use provcfg::{Config, Configurable, DynamicSource, Error, LiveConfig};
    ///
    /// # #[derive(Configurable, serde::Deserialize, Clone, Default)]
    /// # struct Settings { port: u16 }
    /// let rows = DynamicSource::new("db", KeyValueSource::new("db", [("port", "80")]));
    /// let live = LiveConfig::<SettingsProv>::with_validator(
    ///     Config::new().add_source(rows.clone()),
    ///     |config| match config.port.value() {
    ///         0 => Err("port must not be 0"),
    ///         _ => Ok(()),
    ///     },
    /// )
    /// .unwrap();
    ///
    /// rows.replace(KeyValueSource::new("db", [("port", "0")]));
    /// assert!(matches!(live.reload(), Err(Error::Invalid(_))));
    /// assert_eq!(live.load().port.value(), &80);
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Deserialize`] if a source's data does not fit `T`,
    /// [`Error::Invalid`] if `validator` rejects the config.
    pub fn with_validator<E: Into<BoxError>>(
        config: Config,
        validator: impl Fn(&T) -> Result<(), E> + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        Self::create(
            config,
            Some(Box::new(move |config| {
                validator(config).map_err(Into::into)
            })),
        )
    }

    fn create(config: Config, validator: Option<Validator<T>>) -> Result<Self, Error> {
        let first = Self::checked_build(&config, validator.as_ref())?;
        Ok(Self {
            config,
            current: ArcSwap::from_pointee(first),
            reload_lock: Mutex::new(()),
            callbacks: Mutex::new(Vec::new()),
            validator,
        })
    }

    fn checked_build(config: &Config, validator: Option<&Validator<T>>) -> Result<T, Error> {
        let built = config.build::<T>()?;
        if let Some(validator) = validator {
            validator(&built).map_err(Error::Invalid)?;
        }
        Ok(built)
    }

    /// Build the config as it would be after `dynamic.replace(data)` and a
    /// reload, without changing either: the current config, the source and
    /// other threads are untouched. Use it to check data before storing it,
    /// for example rows inside a transaction that is committed only if this
    /// succeeds. `dynamic` must be one of this config's sources; otherwise
    /// `data` is ignored.
    ///
    /// # Errors
    ///
    /// The errors of [`reload`](Self::reload).
    pub fn preview<S: Source + Send + Sync + 'static>(
        &self,
        dynamic: &DynamicSource,
        data: S,
    ) -> Result<T, Error> {
        let data: SourceArc = Arc::new(data);
        PREVIEWS.with(|previews| previews.borrow_mut().push((dynamic.slot(), data)));
        let _guard = PreviewGuard;
        Self::checked_build(&self.config, self.validator.as_ref())
    }

    /// The current config. Cheap and lock-free; the snapshot stays valid and
    /// unchanged even if a reload happens while it is held.
    pub fn load(&self) -> Arc<T> {
        self.current.load_full()
    }

    /// Build the config again and make it current. Returns the dotted paths of
    /// the leaves whose value or source [`Category`] changed; a change hidden
    /// by a higher-priority source is not reported, but is recorded in that
    /// leaf's [`history`](crate::ValueHistory::history). Callbacks registered
    /// with [`on_change`](Self::on_change) run before this returns, and only
    /// when something changed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Deserialize`] if a source's data does not fit `T`, or
    /// [`Error::Invalid`] if the validator rejects it. The previous config
    /// stays current and no callback runs.
    pub fn reload(&self) -> Result<Vec<String>, Error> {
        let _guard = self
            .reload_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let new = Arc::new(Self::checked_build(&self.config, self.validator.as_ref())?);
        let old = self.current.swap(new.clone());
        let changed = changed_paths(&*old, &*new);
        if !changed.is_empty() {
            self.notify(&Change {
                old,
                new,
                changed: changed.clone(),
            });
        }
        Ok(changed)
    }

    /// Run `callback` after every reload that changed something.
    ///
    /// Callbacks run on the thread that called [`reload`](Self::reload), in
    /// registration order. A callback must not call `reload` itself, which
    /// would deadlock.
    pub fn on_change(&self, callback: impl Fn(&Change<T>) + Send + Sync + 'static) {
        self.callbacks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::new(callback));
    }

    fn notify(&self, change: &Change<T>) {
        // Clone the list so a callback may register further callbacks.
        let callbacks = self
            .callbacks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for callback in callbacks {
            callback(change);
        }
    }
}

type LeafSnapshot = BTreeMap<String, (serde_json::Value, Category)>;

fn changed_paths<T: Provenance>(old: &T, new: &T) -> Vec<String> {
    let old = snapshot(old);
    snapshot(new)
        .into_iter()
        .filter(|(path, leaf)| old.get(path) != Some(leaf))
        .map(|(path, _)| path)
        .collect()
}

fn snapshot<T: Provenance>(config: &T) -> LeafSnapshot {
    let mut leaves = BTreeMap::new();
    config.walk_leaves("", &mut |path, value, category, _secret| {
        // A value serde_json cannot represent (a map with non-string keys)
        // compares as null, so only its category change is detected.
        let value = serde_json::to_value(value).unwrap_or_default();
        leaves.insert(path.to_string(), (value, category));
    });
    leaves
}
