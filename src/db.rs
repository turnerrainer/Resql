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
        // Server-side query-timeout. The variable name AND units
        // differ between MySQL and MariaDB — we have to detect the
        // server at connect time:
        //
        //   MySQL 5.7.8+ : SET SESSION MAX_EXECUTION_TIME = <ms>
        //                  (INT milliseconds, SELECT-only, hint-shaped)
        //   MariaDB 10.1+: SET SESSION max_statement_time = <seconds>
        //                  (DOUBLE seconds, applies to all statements)
        //
        // Sending the wrong one gives `1193 Unknown system variable`
        // and takes the whole pool down. Older servers (MySQL <5.7.8,
        // MariaDB <10.1) have no equivalent at all — we log a WARN
        // and let the pool come up; the middleware `TimeoutLayer`
        // remains the authoritative wall-clock cap either way.
        let statement_timeout_ms = statement_timeout_secs.map(|s| s.saturating_mul(1000));
        let mut pool_opts = MySqlPoolOptions::new()
            .max_connections(ds.max_connections)
            .acquire_timeout(Duration::from_secs(ds.acquire_timeout_seconds));
        if let Some(ms) = statement_timeout_ms {
            let ds_name = ds.name.clone();
            pool_opts = pool_opts.after_connect(move |conn, _meta| {
                let ds_name = ds_name.clone();
                Box::pin(async move {
                    let version: String = sqlx::query_scalar("SELECT VERSION()")
                        .fetch_one(&mut *conn)
                        .await
                        .unwrap_or_default();
                    let sql = mysql_timeout_sql(&version, ms);
                    if let Some(sql) = sql {
                        if let Err(e) = conn.execute(sql.as_str()).await {
                            tracing::warn!(
                                datasource = %ds_name,
                                server_version = %version,
                                error = %e,
                                "server rejected query-timeout variable; relying on \
                                 middleware TimeoutLayer only (defence-in-depth skipped)"
                            );
                        }
                    } else {
                        tracing::warn!(
                            datasource = %ds_name,
                            server_version = %version,
                            "server version has no supported query-timeout variable \
                             (MySQL <5.7.8 / MariaDB <10.1); relying on middleware \
                             TimeoutLayer only"
                        );
                    }
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

/// Pick the right `SET SESSION` statement for a MySQL / MariaDB
/// server given its `VERSION()` string and the requested timeout
/// in milliseconds.
///
/// Returns `None` when the server is too old to support any
/// query-timeout variable (MySQL <5.7.8, MariaDB <10.1). The caller
/// logs a WARN and falls back to the middleware TimeoutLayer only.
///
/// Rules:
///  - `VERSION()` contains "mariadb" (case-insensitive) → MariaDB
///    path. Uses `max_statement_time` (seconds, DOUBLE). Introduced
///    in MariaDB 10.1.1 (2015). We skip only on versions whose
///    leading semver parses as <10.1 — the "5.5.5-" compat prefix
///    some MariaDB builds add is handled by taking the version
///    number that appears BEFORE "-MariaDB".
///  - Otherwise → MySQL path. Uses `MAX_EXECUTION_TIME`
///    (milliseconds, INT). Introduced in MySQL 5.7.8. Skip on
///    older versions.
///
/// This is a pure function so unit tests can exercise every
/// branch without a live server. The connect-time code in
/// `connect()` calls it on every connection, so the version
/// detection cost is one `SELECT VERSION()` per pool acquisition
/// (negligible).
fn mysql_timeout_sql(version: &str, ms: u64) -> Option<String> {
    let v_lower = version.to_ascii_lowercase();
    if v_lower.contains("mariadb") {
        if mariadb_supports_statement_timeout(version) {
            let seconds = (ms as f64) / 1000.0;
            Some(format!("SET SESSION max_statement_time = {seconds}"))
        } else {
            None
        }
    } else if mysql_supports_max_execution_time(version) {
        Some(format!("SET SESSION MAX_EXECUTION_TIME = {ms}"))
    } else {
        None
    }
}

/// `MAX_EXECUTION_TIME` landed in MySQL 5.7.8 (2015). Earlier
/// versions reject the variable with error 1193.
fn mysql_supports_max_execution_time(version: &str) -> bool {
    match parse_leading_version(version) {
        Some((major, minor, patch)) => (major, minor, patch) >= (5, 7, 8),
        // Unparseable → assume modern. Keeps the common case (any
        // sqlx-mysql-reachable server) working; the only cost of a
        // false positive is a WARN + fallback at connect.
        None => true,
    }
}

/// `max_statement_time` landed in MariaDB 10.1.1 (2015). Older
/// MariaDB (10.0.x and earlier) has no equivalent.
///
/// MariaDB 10.x versions commonly return VERSION() shaped like
/// `"10.5.21-MariaDB"` (modern) or `"5.5.5-10.11.6-MariaDB"` (older
/// builds that prefixed "5.5.5-" for backwards compat with legacy
/// MySQL clients). The `"5.5.5-"` prefix must be stripped before
/// parsing — otherwise we'd think a MariaDB 10.11 server is a
/// MySQL 5.5 server and skip the timeout.
fn mariadb_supports_statement_timeout(version: &str) -> bool {
    let stripped = version.strip_prefix("5.5.5-").unwrap_or(version);
    match parse_leading_version(stripped) {
        Some((major, minor, patch)) => (major, minor, patch) >= (10, 1, 1),
        None => true,
    }
}

/// Parse the leading `MAJOR.MINOR.PATCH` from a server version
/// string. Stops at the first non-digit, non-dot character.
///
/// Examples:
///   "8.0.33"                → Some((8, 0, 33))
///   "5.7.44-log"            → Some((5, 7, 44))
///   "10.5.21-MariaDB-ubu20" → Some((10, 5, 21))
///   "8.0"                   → None (not enough components)
///   ""                      → None
fn parse_leading_version(s: &str) -> Option<(u64, u64, u64)> {
    let head: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = head.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_leading_version_happy_paths() {
        assert_eq!(parse_leading_version("8.0.33"), Some((8, 0, 33)));
        assert_eq!(parse_leading_version("5.7.44-log"), Some((5, 7, 44)));
        assert_eq!(
            parse_leading_version("10.5.21-MariaDB-ubu2004"),
            Some((10, 5, 21))
        );
        assert_eq!(parse_leading_version("8.4.0"), Some((8, 4, 0)));
    }

    #[test]
    fn parse_leading_version_rejects_malformed() {
        assert_eq!(parse_leading_version(""), None);
        assert_eq!(parse_leading_version("8.0"), None);
        assert_eq!(parse_leading_version("not-a-version"), None);
    }

    #[test]
    fn mysql_timeout_sql_picks_mariadb_variant_by_substring() {
        // Core user-reported bug: MariaDB rejects MAX_EXECUTION_TIME
        // with error 1193. Must emit `max_statement_time` instead,
        // in SECONDS not milliseconds.
        let sql = mysql_timeout_sql("10.5.21-MariaDB-ubu2004", 30_000);
        assert_eq!(sql.as_deref(), Some("SET SESSION max_statement_time = 30"));
    }

    #[test]
    fn mysql_timeout_sql_handles_mariadb_55_compat_prefix() {
        // Some MariaDB builds return "5.5.5-10.11.6-MariaDB" for
        // backwards-compat with ancient MySQL clients. Naive leading-
        // semver parse would read 5.5.5 and skip the timeout as "too
        // old". The strip handles this.
        let sql = mysql_timeout_sql("5.5.5-10.11.6-MariaDB", 10_000);
        assert_eq!(sql.as_deref(), Some("SET SESSION max_statement_time = 10"));
    }

    #[test]
    fn mysql_timeout_sql_mariadb_fractional_seconds() {
        // The variable is DOUBLE. A sub-second timeout (e.g. 500ms)
        // must round-trip as `0.5`, not `0`.
        let sql = mysql_timeout_sql("10.11.6-MariaDB", 500);
        assert_eq!(sql.as_deref(), Some("SET SESSION max_statement_time = 0.5"));
    }

    #[test]
    fn mysql_timeout_sql_skips_ancient_mariadb() {
        // MariaDB <10.1 has no equivalent variable. Return None so
        // the caller logs a WARN and skips cleanly instead of
        // sending a statement the server will reject with 1193.
        assert_eq!(mysql_timeout_sql("10.0.38-MariaDB", 30_000), None);
    }

    #[test]
    fn mysql_timeout_sql_picks_mysql_variant_without_mariadb_substring() {
        let sql = mysql_timeout_sql("8.0.33", 30_000);
        assert_eq!(
            sql.as_deref(),
            Some("SET SESSION MAX_EXECUTION_TIME = 30000")
        );
    }

    #[test]
    fn mysql_timeout_sql_skips_older_mysql_than_5_7_8() {
        // MAX_EXECUTION_TIME landed in 5.7.8. 5.7.7 and earlier
        // return 1193 — skip to avoid tanking the pool.
        assert_eq!(mysql_timeout_sql("5.7.7-log", 30_000), None);
        assert_eq!(mysql_timeout_sql("5.6.51", 30_000), None);
        assert_eq!(mysql_timeout_sql("5.5.62", 30_000), None);
    }

    #[test]
    fn mysql_timeout_sql_mysql_boundary_at_5_7_8() {
        // Exact-boundary test: 5.7.8 must succeed, 5.7.7 must skip.
        assert!(mysql_timeout_sql("5.7.8-log", 30_000).is_some());
        assert_eq!(mysql_timeout_sql("5.7.7-log", 30_000), None);
    }

    #[test]
    fn mysql_timeout_sql_mariadb_boundary_at_10_1_1() {
        // Exact-boundary test for MariaDB: 10.1.1 must succeed,
        // 10.1.0 must skip (that version shipped without the
        // variable; the stable backport was 10.1.1).
        assert!(mysql_timeout_sql("10.1.1-MariaDB", 30_000).is_some());
        assert_eq!(mysql_timeout_sql("10.1.0-MariaDB", 30_000), None);
    }

    #[test]
    fn mysql_timeout_sql_mariadb_detection_is_case_insensitive() {
        // Just in case a future variant returns the token in a
        // different case — "mariadb" / "MariaDB" / "MARIADB" all map
        // to the MariaDB branch.
        for v in ["10.5.0-mariadb", "10.5.0-MariaDB", "10.5.0-MARIADB"] {
            assert!(
                mysql_timeout_sql(v, 30_000)
                    .unwrap_or_default()
                    .contains("max_statement_time"),
                "expected max_statement_time for {v}"
            );
        }
    }

    #[test]
    fn mysql_timeout_sql_unparseable_version_defaults_to_modern() {
        // When we can't parse the version, assume modern. A false
        // positive produces one WARN at connect (operator sees it);
        // a false negative would silently disable the timeout on a
        // server that supports it.
        assert!(mysql_timeout_sql("some-weird-fork", 30_000).is_some());
    }

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
