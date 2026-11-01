use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use arc_swap::ArcSwap;

use crate::{Category, Config, Error, Provenance, Source, SourceArc};

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

    fn inner(&self) -> SourceArc {
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
}

impl<T: Provenance> LiveConfig<T> {
    /// Build `config` once and keep it for later reloads.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Deserialize`] if a source's data does not fit `T`.
    pub fn new(config: Config) -> Result<Self, Error> {
        let current = ArcSwap::from_pointee(config.build::<T>()?);
        Ok(Self {
            config,
            current,
            reload_lock: Mutex::new(()),
            callbacks: Mutex::new(Vec::new()),
        })
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
    /// Returns [`Error::Deserialize`] if a source's data does not fit `T`. The
    /// previous config stays current and no callback runs.
    pub fn reload(&self) -> Result<Vec<String>, Error> {
        let _guard = self
            .reload_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let new = Arc::new(self.config.build::<T>()?);
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
