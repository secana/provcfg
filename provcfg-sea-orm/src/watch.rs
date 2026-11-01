use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use provcfg::Provenance;

use crate::{Error, LiveSettings, revision};

type ErrorCallback = Arc<dyn Fn(&Error) + Send + Sync>;

/// Settings for [`LiveSettings::watch`].
#[derive(Clone)]
pub struct WatchOptions {
    poll_interval: Duration,
    on_error: Option<ErrorCallback>,
}

impl WatchOptions {
    /// Poll every 5 seconds and ignore errors.
    #[must_use]
    pub fn new() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            on_error: None,
        }
    }

    /// How often to compare the namespace revision. On SQLite this is the only
    /// way to notice changes; on PostgreSQL it is the fallback for missed
    /// notifications.
    #[must_use]
    pub fn poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Called for every error the watcher hits and survives, such as a lost
    /// connection or rows that do not fit the config type.
    #[must_use]
    pub fn on_error(mut self, callback: impl Fn(&Error) + Send + Sync + 'static) -> Self {
        self.on_error = Some(Arc::new(callback));
        self
    }

    fn report(&self, error: &Error) {
        if let Some(callback) = &self.on_error {
            callback(error);
        }
    }
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Provenance + Send + Sync + 'static> LiveSettings<T> {
    /// A future that keeps this config in sync with the database until it is
    /// dropped. Spawn it on a tokio runtime:
    ///
    /// ```ignore
    /// tokio::spawn(settings.watch(WatchOptions::new()));
    /// ```
    ///
    /// On PostgreSQL it listens for the notifications sent by the triggers of
    /// [`TrackRevisions`](crate::migration::TrackRevisions), reloads right
    /// away, and still polls as a fallback. After a lost connection it
    /// reconnects and checks the revision, so missed notifications are not
    /// lost. On SQLite it polls the revision. Changes reach
    /// [`on_change`](provcfg::LiveConfig::on_change) callbacks as usual.
    pub fn watch(&self, options: WatchOptions) -> impl Future<Output = ()> + Send + 'static {
        let watcher = Watcher {
            settings: self.clone(),
            seen: self.revision(),
            options,
        };
        watcher.run()
    }
}

struct Watcher<T> {
    settings: LiveSettings<T>,
    /// Last revision acted on, even if applying it failed, so invalid data is
    /// reported once per change instead of on every poll.
    seen: i64,
    options: WatchOptions,
}

impl<T: Provenance + Send + Sync + 'static> Watcher<T> {
    async fn run(mut self) {
        #[cfg(feature = "postgres")]
        if self.settings.db().get_database_backend() == sea_orm::DbBackend::Postgres {
            return self.listen().await;
        }
        loop {
            tokio::time::sleep(self.options.poll_interval).await;
            self.check().await;
        }
    }

    /// Refresh if the namespace revision moved since the last check.
    async fn check(&mut self) {
        let result = async {
            let current = revision(self.settings.db(), self.settings.namespace()).await?;
            if current != self.seen {
                self.seen = current;
                self.settings.refresh().await?;
            }
            Ok::<_, Error>(())
        }
        .await;
        if let Err(error) = result {
            self.options.report(&error);
        }
    }

    #[cfg(feature = "postgres")]
    async fn listen(mut self) {
        use sea_orm::sqlx::postgres::PgListener;

        loop {
            let pool = self.settings.db().get_postgres_connection_pool();
            let mut listener = match PgListener::connect_with(pool).await {
                Ok(listener) => listener,
                Err(error) => {
                    self.report_sqlx(error);
                    tokio::time::sleep(self.options.poll_interval).await;
                    continue;
                }
            };
            if let Err(error) = listener.listen(crate::NOTIFY_CHANNEL).await {
                self.report_sqlx(error);
                tokio::time::sleep(self.options.poll_interval).await;
                continue;
            }
            // Catch up on anything that happened while not listening.
            self.check().await;
            loop {
                match tokio::time::timeout(self.options.poll_interval, listener.try_recv()).await {
                    Ok(Ok(Some(notification))) => {
                        if notification.payload() == self.settings.namespace() {
                            self.check().await;
                        }
                    }
                    // The connection was lost; the next `try_recv` reconnects.
                    // Notifications in between are gone, so check now.
                    Ok(Ok(None)) | Err(_) => self.check().await,
                    Ok(Err(error)) => {
                        self.report_sqlx(error);
                        break;
                    }
                }
            }
        }
    }

    #[cfg(feature = "postgres")]
    fn report_sqlx(&self, error: sea_orm::sqlx::Error) {
        self.options
            .report(&Error::Db(sea_orm::DbErr::Custom(error.to_string())));
    }
}
