use serde_json::{Map, Value};
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
use sqlx::{Column, Row, TypeInfo};

use crate::db::Pool;
use crate::error::ResqlError;

/// Convert a Java-style named-parameter SQL (`:name`) into positional
/// placeholders for the target dialect, returning the rewritten SQL and the
/// ordered list of parameter names that must be bound.
///
/// Skips characters inside `'...'` / `"..."` string literals, `--` and
/// `/* ... */` comments, and `::` casts (Postgres).
pub fn rewrite_named_params(sql: &str, dialect: Dialect) -> (String, Vec<String>) {
    let mut out = String::with_capacity(sql.len());
    let mut params: Vec<String> = Vec::new();
    let bytes = sql.as_bytes();
    let mut i = 0usize;
    let n = bytes.len();

    while i < n {
        let c = bytes[i] as char;

        // Line comment
        if c == '-' && i + 1 < n && bytes[i + 1] == b'-' {
            while i < n && bytes[i] != b'\n' {
                out.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }
        // Block comment
        if c == '/' && i + 1 < n && bytes[i + 1] == b'*' {
            out.push_str("/*");
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                out.push(bytes[i] as char);
                i += 1;
            }
            if i + 1 < n {
                out.push_str("*/");
                i += 2;
            }
            continue;
        }
        // String literal (single or double quoted). Doubled quote inside = escape.
        if c == '\'' || c == '"' {
            let quote = c;
            out.push(quote);
            i += 1;
            while i < n {
                let ch = bytes[i] as char;
                out.push(ch);
                i += 1;
                if ch == quote {
                    if i < n && bytes[i] == quote as u8 {
                        out.push(quote);
                        i += 1;
                        continue;
                    }
                    break;
                }
            }
            continue;
        }
        // Postgres cast `::` — not a parameter.
        if c == ':' && i + 1 < n && bytes[i + 1] == b':' {
            out.push_str("::");
            i += 2;
            continue;
        }
        // Named parameter `:ident`
        if c == ':' && i + 1 < n && is_ident_start(bytes[i + 1] as char) {
            let start = i + 1;
            let mut end = start;
            while end < n && is_ident_cont(bytes[end] as char) {
                end += 1;
            }
            let name = std::str::from_utf8(&bytes[start..end]).unwrap().to_string();
            let position = match params.iter().position(|p| p == &name) {
                Some(p) => p,
                None => {
                    params.push(name.clone());
                    params.len() - 1
                }
            };
            match dialect {
                Dialect::Postgres => out.push_str(&format!("${}", position + 1)),
                Dialect::Sqlite => out.push_str(&format!("?{}", position + 1)),
            }
            i = end;
            continue;
        }
        out.push(c);
        i += 1;
    }
    (out, params)
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_cont(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[derive(Copy, Clone, Debug)]
pub enum Dialect {
    Postgres,
    Sqlite,
}

impl From<&Pool> for Dialect {
    fn from(p: &Pool) -> Self {
        match p {
            Pool::Postgres(_) => Dialect::Postgres,
            Pool::Sqlite(_) => Dialect::Sqlite,
        }
    }
}

/// Execute a saved SQL against the given pool with a JSON parameter map.
///
/// Returns an array of row objects. If the statement has no result set
/// (DDL, empty result), returns an empty array — matching Java Resql behavior.
pub async fn execute(
    pool: &Pool,
    sql: &str,
    params: &Value,
) -> Result<Vec<Map<String, Value>>, ResqlError> {
    let dialect = Dialect::from(pool);
    let (rewritten, param_names) = rewrite_named_params(sql, dialect);

    let param_map = match params {
        Value::Object(m) => m.clone(),
        Value::Null => Map::new(),
        _ => {
            return Err(ResqlError::MalformedRequest(
                "request body must be a JSON object or null".into(),
            ));
        }
    };

    // Missing-parameter check (matches Java NamedParameterJdbcTemplate behavior).
    for name in &param_names {
        if !param_map.contains_key(name) {
            return Err(ResqlError::MissingParameter(name.clone()));
        }
    }

    match pool {
        Pool::Postgres(pg) => execute_pg(pg, &rewritten, &param_names, &param_map).await,
        Pool::Sqlite(sq) => execute_sqlite(sq, &rewritten, &param_names, &param_map).await,
    }
}

async fn execute_pg(
    pool: &sqlx::PgPool,
    sql: &str,
    names: &[String],
    params: &Map<String, Value>,
) -> Result<Vec<Map<String, Value>>, ResqlError> {
    let mut q = sqlx::query(sql);
    for name in names {
        let v = params.get(name).cloned().unwrap_or(Value::Null);
        q = bind_pg(q, v);
    }
    let rows = q
        .fetch_all(pool)
        .await
        .map_err(|e| ResqlError::SqlExecution(e.to_string()))?;
    Ok(rows.iter().map(pg_row_to_json).collect())
}

async fn execute_sqlite(
    pool: &sqlx::SqlitePool,
    sql: &str,
    names: &[String],
    params: &Map<String, Value>,
) -> Result<Vec<Map<String, Value>>, ResqlError> {
    let mut q = sqlx::query(sql);
    for name in names {
        let v = params.get(name).cloned().unwrap_or(Value::Null);
        q = bind_sqlite(q, v);
    }
    let rows = q
        .fetch_all(pool)
        .await
        .map_err(|e| ResqlError::SqlExecution(e.to_string()))?;
    Ok(rows.iter().map(sqlite_row_to_json).collect())
}

fn bind_pg<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    v: Value,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match v {
        Value::Null => q.bind(Option::<String>::None),
        Value::Bool(b) => q.bind(b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i)
            } else if let Some(u) = n.as_u64() {
                q.bind(u as i64)
            } else if let Some(f) = n.as_f64() {
                q.bind(f)
            } else {
                q.bind(n.to_string())
            }
        }
        Value::String(s) => q.bind(s),
        Value::Array(_) | Value::Object(_) => q.bind(sqlx::types::Json(v)),
    }
}

fn bind_sqlite<'q>(
    q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    v: Value,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    match v {
        Value::Null => q.bind(Option::<String>::None),
        Value::Bool(b) => q.bind(b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i)
            } else if let Some(f) = n.as_f64() {
                q.bind(f)
            } else {
                q.bind(n.to_string())
            }
        }
        Value::String(s) => q.bind(s),
        Value::Array(_) | Value::Object(_) => q.bind(v.to_string()),
    }
}

fn pg_row_to_json(row: &PgRow) -> Map<String, Value> {
    let mut out = Map::new();
    for (idx, col) in row.columns().iter().enumerate() {
        let key = snake_to_camel(col.name());
        let value = pg_column_value(row, idx, col.type_info().name());
        out.insert(key, value);
    }
    out
}

fn sqlite_row_to_json(row: &SqliteRow) -> Map<String, Value> {
    let mut out = Map::new();
    for (idx, col) in row.columns().iter().enumerate() {
        let key = snake_to_camel(col.name());
        let value = sqlite_column_value(row, idx, col.type_info().name());
        out.insert(key, value);
    }
    out
}

fn pg_column_value(row: &PgRow, idx: usize, ty: &str) -> Value {
    let ty_upper = ty.to_ascii_uppercase();
    // Try nullable first — if the column is NULL, short-circuit.
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(idx) {
        if ty_upper == "TEXT"
            || ty_upper == "VARCHAR"
            || ty_upper == "CHAR"
            || ty_upper == "BPCHAR"
            || ty_upper == "NAME"
            || ty_upper == "CITEXT"
            || ty_upper == "UUID"
        {
            return Value::String(v);
        }
    }
    if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
        if ty_upper == "BOOL" || ty_upper == "BOOLEAN" {
            return Value::Bool(v);
        }
    }
    // Integer types: sqlx decodes each PG width to its native Rust type
    // (INT2→i16, INT4→i32, INT8→i64). Pick the right accessor per width;
    // widening cast to i64 for the JSON representation.
    if matches!(ty_upper.as_str(), "INT2" | "SMALLINT") {
        if let Ok(Some(v)) = row.try_get::<Option<i16>, _>(idx) {
            return Value::Number(i64::from(v).into());
        }
    }
    if matches!(ty_upper.as_str(), "INT4" | "INT" | "INTEGER") {
        if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(idx) {
            return Value::Number(i64::from(v).into());
        }
    }
    if matches!(ty_upper.as_str(), "INT8" | "BIGINT" | "OID") {
        if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
            return Value::Number(v.into());
        }
    }
    if matches!(
        ty_upper.as_str(),
        "FLOAT4" | "REAL" | "FLOAT8" | "DOUBLE PRECISION"
    ) {
        if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
            return serde_json::Number::from_f64(v)
                .map(Value::Number)
                .unwrap_or(Value::Null);
        }
    }
    // NUMERIC → serialised as string so callers don't silently truncate via
    // f64. Requires the `rust_decimal` sqlx feature.
    if matches!(ty_upper.as_str(), "NUMERIC" | "DECIMAL") {
        if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::Decimal>, _>(idx) {
            return Value::String(v.to_string());
        }
    }
    // JSON / JSONB
    if matches!(ty_upper.as_str(), "JSON" | "JSONB") {
        if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::Json<Value>>, _>(idx) {
            return v.0;
        }
    }
    // Timestamps and dates: stringify.
    if matches!(
        ty_upper.as_str(),
        "TIMESTAMP" | "TIMESTAMPTZ" | "DATE" | "TIME" | "TIMETZ"
    ) {
        if let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveDateTime>, _>(idx) {
            return Value::String(v.to_string());
        }
        if let Ok(Some(v)) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(idx) {
            return Value::String(v.to_rfc3339());
        }
        if let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveDate>, _>(idx) {
            return Value::String(v.to_string());
        }
    }
    // UUID
    if ty_upper == "UUID" {
        if let Ok(Some(v)) = row.try_get::<Option<uuid::Uuid>, _>(idx) {
            return Value::String(v.to_string());
        }
    }
    // Text array — best-effort as string list.
    if ty_upper.starts_with("_TEXT") || ty_upper == "TEXT[]" {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<String>>, _>(idx) {
            return Value::Array(v.into_iter().map(Value::String).collect());
        }
    }
    if ty_upper.starts_with("_INT") || ty_upper.ends_with("[]") {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<i64>>, _>(idx) {
            return Value::Array(v.into_iter().map(|i| Value::Number(i.into())).collect());
        }
    }
    // Explicit NULL detection: try any nullable type.
    if row
        .try_get::<Option<String>, _>(idx)
        .map(|v| v.is_none())
        .unwrap_or(false)
    {
        return Value::Null;
    }
    // Last-resort text.
    match row.try_get::<Option<String>, _>(idx) {
        Ok(Some(v)) => Value::String(v),
        Ok(None) => Value::Null,
        Err(_) => Value::Null,
    }
}

fn sqlite_column_value(row: &SqliteRow, idx: usize, ty: &str) -> Value {
    // SQLite is dynamically typed: expression columns often carry an empty
    // type name. Try type-informed extraction first, then fall back to
    // probing the value in the order Java would produce (integer → real →
    // text → bool). NULL surfaces as Value::Null regardless.
    let ty_upper = ty.to_ascii_uppercase();
    if ty_upper.contains("INT") {
        if let Ok(v) = row.try_get::<Option<i64>, _>(idx) {
            return v.map(|n| Value::Number(n.into())).unwrap_or(Value::Null);
        }
    }
    if ty_upper.contains("REAL") || ty_upper.contains("FLOA") || ty_upper.contains("DOUB") {
        if let Ok(v) = row.try_get::<Option<f64>, _>(idx) {
            return v
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null);
        }
    }
    if ty_upper.contains("BOOL") {
        if let Ok(v) = row.try_get::<Option<bool>, _>(idx) {
            return v.map(Value::Bool).unwrap_or(Value::Null);
        }
    }
    // Untyped fallback: probe common types in order.
    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
        return Value::Number(v.into());
    }
    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
        if let Some(n) = serde_json::Number::from_f64(v) {
            return Value::Number(n);
        }
    }
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(idx) {
        return Value::String(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
        return Value::String(String::from_utf8_lossy(&v).into_owned());
    }
    Value::Null
}

/// snake_case → camelCase; leaves single tokens (`id`) alone.
pub fn snake_to_camel(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut upper_next = false;
    for c in lower.chars() {
        if c == '_' {
            upper_next = true;
        } else if upper_next {
            out.push(c.to_ascii_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snake_to_camel_basics() {
        assert_eq!(snake_to_camel("id"), "id");
        assert_eq!(snake_to_camel("user_id"), "userId");
        assert_eq!(snake_to_camel("PASSWORD_HASH"), "passwordHash");
        assert_eq!(snake_to_camel("a_b_c_d"), "aBCD");
        assert_eq!(snake_to_camel("_leading"), "Leading");
    }

    #[test]
    fn rewrite_simple_named_param() {
        let (sql, params) =
            rewrite_named_params("SELECT * FROM t WHERE id = :id", Dialect::Postgres);
        assert_eq!(sql, "SELECT * FROM t WHERE id = $1");
        assert_eq!(params, vec!["id"]);
    }

    #[test]
    fn rewrite_repeated_param_reuses_position() {
        let (sql, params) = rewrite_named_params("SELECT :x, :x, :y", Dialect::Postgres);
        assert_eq!(sql, "SELECT $1, $1, $2");
        assert_eq!(params, vec!["x", "y"]);
    }

    #[test]
    fn rewrite_ignores_pg_cast() {
        let (sql, params) = rewrite_named_params("SELECT :val::int", Dialect::Postgres);
        assert_eq!(sql, "SELECT $1::int");
        assert_eq!(params, vec!["val"]);
    }

    #[test]
    fn rewrite_ignores_string_contents() {
        let (sql, params) = rewrite_named_params(
            "SELECT ':not_a_param' FROM t WHERE id = :id",
            Dialect::Postgres,
        );
        assert_eq!(sql, "SELECT ':not_a_param' FROM t WHERE id = $1");
        assert_eq!(params, vec!["id"]);
    }

    #[test]
    fn rewrite_ignores_line_comment() {
        let (sql, params) = rewrite_named_params("-- :fake\nSELECT :real", Dialect::Postgres);
        assert_eq!(sql, "-- :fake\nSELECT $1");
        assert_eq!(params, vec!["real"]);
    }

    #[test]
    fn rewrite_ignores_block_comment() {
        let (sql, params) = rewrite_named_params("/* :fake */ SELECT :real", Dialect::Postgres);
        assert_eq!(sql, "/* :fake */ SELECT $1");
        assert_eq!(params, vec!["real"]);
    }

    #[test]
    fn rewrite_ignores_doubled_quote_in_string() {
        let (sql, params) =
            rewrite_named_params("SELECT 'it''s :fake' AS a, :real AS b", Dialect::Postgres);
        assert_eq!(sql, "SELECT 'it''s :fake' AS a, $1 AS b");
        assert_eq!(params, vec!["real"]);
    }

    #[test]
    fn rewrite_sqlite_placeholders() {
        let (sql, params) = rewrite_named_params("SELECT :a, :b", Dialect::Sqlite);
        assert_eq!(sql, "SELECT ?1, ?2");
        assert_eq!(params, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn missing_param_returns_named_error() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let err = execute(&Pool::Sqlite(pool), "SELECT :missing_key AS x", &json!({}))
            .await
            .unwrap_err();
        match err {
            ResqlError::MissingParameter(name) => assert_eq!(name, "missing_key"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn extra_params_ignored() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let rows = execute(
            &Pool::Sqlite(pool),
            "SELECT :used AS u",
            &json!({"used": 1, "unused": "x"}),
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("u").unwrap(), &json!(1));
    }

    #[tokio::test]
    async fn ddl_returns_empty_array() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let rows = execute(
            &Pool::Sqlite(pool),
            "CREATE TABLE x (id INTEGER)",
            &json!({}),
        )
        .await
        .unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn snake_columns_are_camelised() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let rows = execute(
            &Pool::Sqlite(pool.clone()),
            "SELECT 1 AS user_id, 'x' AS password_hash",
            &json!({}),
        )
        .await
        .unwrap();
        assert!(rows[0].contains_key("userId"));
        assert!(rows[0].contains_key("passwordHash"));
    }

    #[tokio::test]
    async fn null_param_binds_as_null() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let rows = execute(
            &Pool::Sqlite(pool),
            "SELECT :v IS NULL AS is_null",
            &json!({"v": Value::Null}),
        )
        .await
        .unwrap();
        assert_eq!(rows[0].get("isNull").unwrap(), &json!(1));
    }

    #[tokio::test]
    async fn body_must_be_object_or_null() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let err = execute(&Pool::Sqlite(pool), "SELECT 1", &json!("not an object"))
            .await
            .unwrap_err();
        assert!(matches!(err, ResqlError::MalformedRequest(_)));
    }
}
