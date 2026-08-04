use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{PgPool, SqlitePool};

use crate::config::DatasourceConfig;
use crate::error::ResqlError;

/// Dialect-typed pool. Chosen once at connect time based on the URL scheme.
#[derive(Debug, Clone)]
pub enum Pool {
    Postgres(PgPool),
    Sqlite(SqlitePool),
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
        let mut reg = Self::default();
        for ds in configs {
            let pool = connect(ds).await?;
            reg.insert(ds.name.clone(), pool);
        }
        Ok(reg)
    }
}

pub async fn connect(ds: &DatasourceConfig) -> Result<Pool, ResqlError> {
    let url = ds.resolved_url()?;
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        let opts: PgConnectOptions = url.parse().map_err(|e| {
            ResqlError::Internal(format!("bad Postgres URL for '{}': {e}", ds.name))
        })?;
        let pool = PgPoolOptions::new()
            .max_connections(ds.max_connections)
            .acquire_timeout(Duration::from_secs(ds.acquire_timeout_seconds))
            .connect_with(opts)
            .await
            .map_err(|e| {
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
    } else {
        Err(ResqlError::Internal(format!(
            "unsupported datasource URL scheme for '{}': must start with postgres://, postgresql://, or sqlite:",
            ds.name
        )))
    }
}

pub type SharedRegistry = Arc<DatasourceRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

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
        let pool = connect(&ds).await.unwrap();
        assert!(matches!(pool, Pool::Sqlite(_)));
    }

    #[tokio::test]
    async fn rejects_unknown_scheme() {
        let ds = DatasourceConfig {
            name: "t".into(),
            url: "mysql://localhost/db".into(),
            username: "".into(),
            password_env: "".into(),
            password: None,
            max_connections: 1,
            acquire_timeout_seconds: 1,
        };
        let err = connect(&ds).await.unwrap_err();
        assert!(err.to_string().contains("unsupported"));
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
}
