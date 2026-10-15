use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, MySqlPool, PgPool, SqlitePool};

use crate::config::DatasourceConfig;
use crate::error::ResqlError;

/// Dialect-typed pool. Chosen once at connect time based on the URL scheme.
#[derive(Debug, Clone)]
pub enum Pool {
    Postgres(PgPool),
    Sqlite(SqlitePool),
    MySql(MySqlPool),
}

/// Registry of datasource-name → Pool.
#[derive(Debug, Clone, Default)]
pub struct DatasourceRegistry {
    pools: HashMap<String, Pool>,
}

impl DatasourceRegistry {
    pub fn get(&self, name: &str) -> Option<&Pool> {
        self.pools.get(name)
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.pools.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn insert(&mut self, name: String, pool: Pool) {
        self.pools.insert(name, pool);
    }

    pub async fn connect_all(configs: &[DatasourceConfig]) -> Result<Self, ResqlError> {
        Self::connect_all_with_timeout(configs, None).await
    }

    /// Gracefully close every pool. Awaits sqlx's `close().await` on
    /// each — pools stop accepting new acquires, in-flight queries
    /// complete, then the underlying connections are closed cleanly.
    ///
    /// Called from `main::run_serve` after `axum::serve`'s graceful
    /// shutdown resolves. Without this, dropping the `Arc` just runs
    /// each pool's Drop impl, which does NOT wait for in-flight I/O
    /// on Postgres (sqlx's `PgPool::close` is the documented way to
    /// signal + await drain).
    pub async fn close_all(&self) {
        for (name, pool) in &self.pools {
            match pool {
                Pool::Postgres(p) => p.close().await,
                Pool::Sqlite(p) => p.close().await,
                Pool::MySql(p) => p.close().await,
            }
            tracing::info!(datasource = %name, "datasource pool closed");
        }
    }

    /// Same as `connect_all`, but sets a server-side query timeout on
    /// every Postgres / MySQL connection so long-running queries are
    /// killed at the database side even if the Rust future outlives
    /// the HTTP timeout. Postgres: `SET statement_timeout = <ms>`
    /// (hard kill). MySQL: `SET SESSION MAX_EXECUTION_TIME = <ms>`
    /// (SELECT-only hint, best-effort — the HTTP-level `TimeoutLayer`
    /// is still the authoritative wall-clock cap). SQLite has no
    /// equivalent server-side kill switch (queries run in-process and
    /// respect the middleware timeout via future cancellation).
    pub async fn connect_all_with_timeout(
        configs: &[DatasourceConfig],
        statement_timeout_secs: Option<u64>,
    ) -> Result<Self, ResqlError> {
        let mut reg = Self::default();
        for ds in configs {
            let pool = connect(ds, statement_timeout_secs).await?;
            // Emit the full (un-redacted) URL at startup so operators
            // still see the connection topology in logs. `/datasources`
            // itself now only exposes the redacted form (R5). Password
            // is stripped from the URL via `mask_password_in_url` so
            // even the log line doesn't hold the secret.
            tracing::info!(
                datasource = %ds.name,
                url = %mask_password_in_url(&ds.url),
                "datasource connected"
            );
            reg.insert(ds.name.clone(), pool);
        }
        Ok(reg)
    }
}

pub async fn connect(
    ds: &DatasourceConfig,
    statement_timeout_secs: Option<u64>,
) -> Result<Pool, ResqlError> {
    let url = ds.resolved_url()?;
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        let opts: PgConnectOptions = url.parse().map_err(|e| {
            ResqlError::Internal(format!("bad Postgres URL for '{}': {e}", ds.name))
        })?;
        // Apply statement_timeout on every connection the pool hands
        // out. Postgres accepts an integer number of milliseconds via
        // `SET`; a value of 0 disables the timeout server-side.
        let statement_timeout_ms = statement_timeout_secs.map(|s| s.saturating_mul(1000));
        let mut pool_opts = PgPoolOptions::new()
            .max_connections(ds.max_connections)
            .acquire_timeout(Duration::from_secs(ds.acquire_timeout_seconds));
        if let Some(ms) = statement_timeout_ms {
            let sql = format!("SET statement_timeout = {ms}");
            pool_opts = pool_opts.after_connect(move |conn, _meta| {
                let sql = sql.clone();
                Box::pin(async move {
                    conn.execute(sql.as_str()).await?;
                    Ok(())
                })
            });
        }
        let pool = pool_opts.connect_with(opts).await.map_err(|e| {
            ResqlError::Internal(format!("cannot connect datasource '{}': {e}", ds.name))
        })?;
        Ok(Pool::Postgres(pool))
    } else if url.starts_with("sqlite:") {
        let opts: SqliteConnectOptions = url
            .parse()
            .map_err(|e| ResqlError::Internal(format!("bad SQLite URL for '{}': {e}", ds.name)))?;
        let opts = opts.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(ds.max_connections)
            .acquire_timeout(Duration::from_secs(ds.acquire_timeout_seconds))
            .connect_with(opts)
            .await
            .map_err(|e| {
                ResqlError::Internal(format!("cannot connect datasource '{}': {e}", ds.name))
            })?;
        Ok(Pool::Sqlite(pool))
    } else if url.starts_with("mysql://") || url.starts_with("mariadb://") {
        let opts: MySqlConnectOptions = url
            .parse()
            .map_err(|e| ResqlError::Internal(format!("bad MySQL URL for '{}': {e}", ds.name)))?;
        // MySQL's analogue to Postgres `statement_timeout` is
        // `MAX_EXECUTION_TIME` — milliseconds, SELECT-only, hint-shaped
        // (the optimizer may still finish past the budget on
        // specific plans). Applied belt-and-braces alongside the
        // middleware `TimeoutLayer`; the latter remains the
        // authoritative wall-clock cap.
        let statement_timeout_ms = statement_timeout_secs.map(|s| s.saturating_mul(1000));
        let mut pool_opts = MySqlPoolOptions::new()
            .max_connections(ds.max_connections)
            .acquire_timeout(Duration::from_secs(ds.acquire_timeout_seconds));
        if let Some(ms) = statement_timeout_ms {
            let sql = format!("SET SESSION MAX_EXECUTION_TIME = {ms}");
            pool_opts = pool_opts.after_connect(move |conn, _meta| {
                let sql = sql.clone();
                Box::pin(async move {
                    conn.execute(sql.as_str()).await?;
                    Ok(())
                })
            });
        }
        let pool = pool_opts.connect_with(opts).await.map_err(|e| {
            ResqlError::Internal(format!("cannot connect datasource '{}': {e}", ds.name))
        })?;
        Ok(Pool::MySql(pool))
    } else {
        Err(ResqlError::Internal(format!(
            "unsupported datasource URL scheme for '{}': must start with postgres://, postgresql://, mysql://, mariadb://, or sqlite:",
            ds.name
        )))
    }
}

pub type SharedRegistry = Arc<DatasourceRegistry>;

/// Replace the password portion of a `scheme://user:pw@host/...` URL
/// with `*****` for logging. Leaves any URL without userinfo (e.g.
/// `sqlite::memory:`) untouched.
fn mask_password_in_url(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let (scheme, rest) = url.split_at(scheme_end + 3);
        if let Some(at) = rest.find('@') {
            let (userinfo, host) = rest.split_at(at);
            if let Some(colon) = userinfo.find(':') {
                let (user, _) = userinfo.split_at(colon);
                return format!("{scheme}{user}:*****{host}");
            }
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_password_in_url_covers_mysql_scheme() {
        // The password masker is scheme-agnostic (operates on userinfo),
        // but the test suite only ever covered sqlite/postgres before.
        // Pin it against mysql:// so a future refactor doesn't skip
        // MySQL URLs on log emission.
        assert_eq!(
            mask_password_in_url("mysql://resql:supersecret@db.internal:3306/app"),
            "mysql://resql:*****@db.internal:3306/app"
        );
        assert_eq!(
            mask_password_in_url("mariadb://user:pw@host:3306/db"),
            "mariadb://user:*****@host:3306/db"
        );
    }

    #[tokio::test]
    async fn connects_sqlite_in_memory() {
        let ds = DatasourceConfig {
            name: "t".into(),
            url: "sqlite::memory:".into(),
            username: "".into(),
            password_env: "".into(),
            password: None,
            max_connections: 1,
            acquire_timeout_seconds: 1,
        };
        let pool = connect(&ds, None).await.unwrap();
        assert!(matches!(pool, Pool::Sqlite(_)));
    }

    #[tokio::test]
    async fn rejects_unknown_scheme() {
        let ds = DatasourceConfig {
            name: "t".into(),
            url: "oracle://localhost/db".into(),
            username: "".into(),
            password_env: "".into(),
            password: None,
            max_connections: 1,
            acquire_timeout_seconds: 1,
        };
        let err = connect(&ds, None).await.unwrap_err();
        assert!(err.to_string().contains("unsupported"));
        // The error message must enumerate every supported scheme so
        // operators can see what's valid without diving into source.
        let msg = err.to_string();
        for sch in [
            "postgres://",
            "postgresql://",
            "mysql://",
            "mariadb://",
            "sqlite:",
        ] {
            assert!(
                msg.contains(sch),
                "expected error to mention scheme `{sch}`, got: {msg}"
            );
        }
    }

    /// MySQL scheme is recognised by the connect function and only
    /// fails when the host is unreachable — not with "unsupported
    /// scheme" (that would mean sqlx-mysql wasn't wired at all).
    #[tokio::test]
    async fn mysql_scheme_recognised() {
        let ds = DatasourceConfig {
            name: "t".into(),
            // Deliberately unreachable; what matters is that we attempt
            // to connect (hit the MySQL branch), not fall through.
            url: "mysql://resql@127.0.0.1:1/db".into(),
            username: "".into(),
            password_env: "".into(),
            password: None,
            max_connections: 1,
            acquire_timeout_seconds: 1,
        };
        let err = connect(&ds, None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("unsupported"),
            "mysql:// must be routed to the MySQL branch, got: {msg}"
        );
    }

    /// `mariadb://` is a documented alias for `mysql://` — same wire
    /// protocol, same sqlx driver — so it must take the same branch.
    #[tokio::test]
    async fn mariadb_scheme_recognised() {
        let ds = DatasourceConfig {
            name: "t".into(),
            url: "mariadb://resql@127.0.0.1:1/db".into(),
            username: "".into(),
            password_env: "".into(),
            password: None,
            max_connections: 1,
            acquire_timeout_seconds: 1,
        };
        let err = connect(&ds, None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("unsupported"),
            "mariadb:// must be routed to the MySQL branch, got: {msg}"
        );
    }

    #[tokio::test]
    async fn registry_get_and_names() {
        let ds = DatasourceConfig {
            name: "a".into(),
            url: "sqlite::memory:".into(),
            username: "".into(),
            password_env: "".into(),
            password: None,
            max_connections: 1,
            acquire_timeout_seconds: 1,
        };
        let reg = DatasourceRegistry::connect_all(&[ds]).await.unwrap();
        assert_eq!(reg.names(), vec!["a".to_string()]);
        assert!(reg.get("a").is_some());
        assert!(reg.get("nope").is_none());
    }

    // T-21: SIGTERM handler audit. Verify that `close_all` marks every
    // pool as closed so a subsequent acquire fails deterministically —
    // this is the observable shape of a graceful drain.
    #[tokio::test]
    async fn close_all_marks_pools_closed() {
        let ds = DatasourceConfig {
            name: "a".into(),
            url: "sqlite::memory:".into(),
            username: "".into(),
            password_env: "".into(),
            password: None,
            max_connections: 1,
            acquire_timeout_seconds: 1,
        };
        let reg = DatasourceRegistry::connect_all(&[ds]).await.unwrap();
        let pool = match reg.get("a").unwrap() {
            Pool::Sqlite(p) => p.clone(),
            _ => unreachable!(),
        };
        assert!(!pool.is_closed(), "pool should be open initially");

        reg.close_all().await;

        assert!(
            pool.is_closed(),
            "pool must be closed after registry.close_all()"
        );
    }
}
