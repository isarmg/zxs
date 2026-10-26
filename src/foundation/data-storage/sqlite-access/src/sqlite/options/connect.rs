use crate::connection::ConnectOptions;
use crate::error::Error;
use crate::executor::Executor;
use crate::sql_str::AssertSqlSafe;
use crate::sqlite::{SqliteConnectOptions, SqliteConnection};
use log::LevelFilter;
use std::fmt::Write;
#[cfg(feature = "url")]
use std::str::FromStr;
use std::time::Duration;
#[cfg(feature = "url")]
use url_encoding::address::Url;

impl ConnectOptions for SqliteConnectOptions {
    type Connection = SqliteConnection;

    #[cfg(feature = "url")]
    fn from_url(url: &Url) -> Result<Self, Error> {
        // SQLite URL parsing is handled specially;
        // we want to treat the following URLs as equivalent:
        //
        // * sqlite:foo.db
        // * sqlite://foo.db
        //
        // If we used `Url::path()`, the latter would return an empty string
        // because `foo.db` gets parsed as the hostname.
        Self::from_str(url.as_str())
    }

    #[cfg(feature = "url")]
    fn to_url_lossy(&self) -> Url {
        self.build_url()
    }

    async fn connect(&self) -> Result<Self::Connection, Error>
    where
        Self::Connection: Sized,
    {
        let mut conn = SqliteConnection::establish(self).await?;

        // Execute PRAGMAs
        conn.execute(AssertSqlSafe(self.pragma_string())).await?;

        if !self.collations.is_empty() {
            let mut locked = conn.lock_handle().await?;

            for collation in &self.collations {
                collation.create(&mut locked.guard.handle)?;
            }
        }

        Ok(conn)
    }

    fn log_statements(mut self, level: LevelFilter) -> Self {
        self.log_settings.log_statements(level);
        self
    }

    fn log_slow_statements(mut self, level: LevelFilter, duration: Duration) -> Self {
        self.log_settings.log_slow_statements(level, duration);
        self
    }
}

impl SqliteConnectOptions {
    /// Collect all `PRAMGA` commands into a single string
    pub(crate) fn pragma_string(&self) -> String {
        let mut string = String::new();

        for (key, opt_value) in &self.pragmas {
            if let Some(value) = opt_value {
                write!(string, "PRAGMA {key} = {value}; ")
                    .expect("invariant: writing to String cannot fail");
            }
        }

        string
    }
}
