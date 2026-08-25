use serde_json::{Map, Value};
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
use sqlx::{Column, Row, TypeInfo};

use crate::db::Pool;
use crate::declaration::{Declaration, ParamType};
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
    declaration: &Declaration,
    params: &Value,
) -> Result<Vec<Map<String, Value>>, ResqlError> {
    let dialect = Dialect::from(pool);
    let (rewritten, param_names) = rewrite_named_params(sql, dialect);
    let raw_map = normalise_params(params)?;
    let param_map = validate_params(declaration, raw_map)?;

    match pool {
        Pool::Postgres(pg) => run_pg(pg, &rewritten, &param_names, &param_map).await,
        Pool::Sqlite(sq) => run_sqlite(sq, &rewritten, &param_names, &param_map).await,
    }
}

/// Execute a saved SQL inside a fresh transaction. Commits on success,
/// rolls back on any error. Used for endpoints marked `-- @transactional`
/// (see `loader::parse_transactional_marker`).
pub async fn execute_transactional(
    pool: &Pool,
    sql: &str,
    declaration: &Declaration,
    params: &Value,
) -> Result<Vec<Map<String, Value>>, ResqlError> {
    let dialect = Dialect::from(pool);
    let (rewritten, param_names) = rewrite_named_params(sql, dialect);
    let raw_map = normalise_params(params)?;
    let param_map = validate_params(declaration, raw_map)?;

    match pool {
        Pool::Postgres(pg) => {
            let mut tx = pg.begin().await.map_err(sql_err)?;
            let result = run_pg(&mut *tx, &rewritten, &param_names, &param_map).await;
            finalise_tx(tx, result).await
        }
        Pool::Sqlite(sq) => {
            let mut tx = sq.begin().await.map_err(sql_err)?;
            let result = run_sqlite(&mut *tx, &rewritten, &param_names, &param_map).await;
            finalise_tx_sqlite(tx, result).await
        }
    }
}

/// Execute N parameter sets against the same SQL atomically. All bind
/// through the same transaction; any failure rolls the whole batch back —
/// no partial writes. Returns one row-set per parameter set on success.
pub async fn execute_batch(
    pool: &Pool,
    sql: &str,
    declaration: &Declaration,
    param_sets: Vec<Value>,
) -> Result<Vec<Vec<Map<String, Value>>>, ResqlError> {
    let dialect = Dialect::from(pool);
    let (rewritten, param_names) = rewrite_named_params(sql, dialect);

    // Normalise + declaration-validate every set BEFORE opening the tx
    // so the "batch of bad requests" case fails fast without hitting the DB.
    let mut normalised: Vec<Map<String, Value>> = Vec::with_capacity(param_sets.len());
    for p in &param_sets {
        let raw_map = normalise_params(p)?;
        normalised.push(validate_params(declaration, raw_map)?);
    }

    match pool {
        Pool::Postgres(pg) => {
            let mut tx = pg.begin().await.map_err(sql_err)?;
            let mut all = Vec::with_capacity(normalised.len());
            for pm in &normalised {
                match run_pg(&mut *tx, &rewritten, &param_names, pm).await {
                    Ok(rows) => all.push(rows),
                    Err(e) => {
                        let _ = tx.rollback().await;
                        return Err(e);
                    }
                }
            }
            tx.commit().await.map_err(sql_err)?;
            Ok(all)
        }
        Pool::Sqlite(sq) => {
            let mut tx = sq.begin().await.map_err(sql_err)?;
            let mut all = Vec::with_capacity(normalised.len());
            for pm in &normalised {
                match run_sqlite(&mut *tx, &rewritten, &param_names, pm).await {
                    Ok(rows) => all.push(rows),
                    Err(e) => {
                        let _ = tx.rollback().await;
                        return Err(e);
                    }
                }
            }
            tx.commit().await.map_err(sql_err)?;
            Ok(all)
        }
    }
}

fn normalise_params(v: &Value) -> Result<Map<String, Value>, ResqlError> {
    match v {
        Value::Object(m) => Ok(m.clone()),
        Value::Null => Ok(Map::new()),
        _ => Err(ResqlError::MalformedRequest(
            "request body must be a JSON object or null".into(),
        )),
    }
}

/// Validate an incoming parameter map against the declaration. Rejects
/// unknown keys, wrong types, and missing required. Fills missing
/// optionals with the declared `default` (or JSON null when no default
/// is set) so the bind layer always sees an entry for every declared
/// param — that's what makes `IS NULL OR col = :x` filters work when
/// the caller omits the key (task 008 / issue #4).
///
/// The returned map contains exactly one entry per declared param, in
/// declaration-agnostic order. Any `:name` a SQL-writer forgot to
/// declare would have failed the file at boot, so the runtime never
/// sees an undeclared `:name` here.
pub fn validate_params(
    declaration: &Declaration,
    mut incoming: Map<String, Value>,
) -> Result<Map<String, Value>, ResqlError> {
    // 1. Reject unknown keys.
    for key in incoming.keys() {
        if !declaration.params.contains_key(key) {
            return Err(ResqlError::UnknownParameter(key.clone()));
        }
    }
    // 2. For every declared param: coerce/validate present values,
    //    fill absent optionals with default (or null), reject missing
    //    required.
    let mut out = Map::with_capacity(declaration.params.len());
    for (name, spec) in &declaration.params {
        if let Some(v) = incoming.remove(name) {
            let coerced =
                coerce_to(spec.ty, v).map_err(|actual| ResqlError::InvalidParameterType {
                    name: name.clone(),
                    expected: spec.ty.as_str(),
                    actual,
                })?;
            // A declared-required param present as JSON null is still
            // "missing" per the semantics we advertise (the null is
            // representationally there, but the caller declared it
            // required so treating null as satisfaction is a footgun).
            if spec.required && coerced.is_null() {
                return Err(ResqlError::MissingParameter(name.clone()));
            }
            // Closed-set validation: reject values outside the declared
            // `enum:` set (JSON Schema semantics — null bypasses; the
            // required/optional gate handles nullability).
            if let Some(allowed) = &spec.allowed {
                if !coerced.is_null() && !allowed.iter().any(|v| v == &coerced) {
                    return Err(ResqlError::InvalidParameterValue {
                        name: name.clone(),
                        value: display_value(&coerced),
                        allowed: allowed.iter().map(display_value).collect(),
                    });
                }
            }
            out.insert(name.clone(), coerced);
        } else if spec.required {
            return Err(ResqlError::MissingParameter(name.clone()));
        } else {
            let filled = spec.default.clone().unwrap_or(Value::Null);
            out.insert(name.clone(), filled);
        }
    }
    Ok(out)
}

/// Render a JSON value for use in error messages: strings unquoted,
/// everything else JSON-encoded (compact, deterministic).
fn display_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Coerce a JSON value to the declared type. Returns Err with a
/// human-readable "actual type" tag on mismatch. Numeric widening
/// (int → number, string-encoded number → integer / number) is
/// permitted; the coercion tries hard within the type family but never
/// crosses families (e.g. no string → bool). Null passes every type
/// check — `required` handling is separate.
pub fn coerce_to(ty: ParamType, v: Value) -> Result<Value, String> {
    match (ty, v) {
        (_, Value::Null) => Ok(Value::Null),
        // string family
        (ParamType::String, Value::String(s))
        | (ParamType::Date, Value::String(s))
        | (ParamType::Datetime, Value::String(s))
        | (ParamType::Uuid, Value::String(s)) => Ok(Value::String(s)),
        // integer: accept i64 as-is; f64 with no fractional part; string
        //          that parses cleanly.
        (ParamType::Integer, Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Number(i.into()))
            } else if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                    Ok(Value::Number((f as i64).into()))
                } else {
                    Err(format!("number {f} is not a whole integer"))
                }
            } else {
                Err("number out of range".into())
            }
        }
        (ParamType::Integer, Value::String(s)) => s
            .parse::<i64>()
            .map(|i| Value::Number(i.into()))
            .map_err(|_| format!("string \"{s}\" is not a valid integer")),
        // number: accept i64 or f64; string that parses.
        (ParamType::Number, Value::Number(n)) => Ok(Value::Number(n)),
        (ParamType::Number, Value::String(s)) => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| format!("string \"{s}\" is not a valid number")),
        // boolean: accept native bool; strings "true"/"false"/"1"/"0".
        (ParamType::Boolean, Value::Bool(b)) => Ok(Value::Bool(b)),
        (ParamType::Boolean, Value::String(s)) => match s.as_str() {
            "true" | "1" => Ok(Value::Bool(true)),
            "false" | "0" => Ok(Value::Bool(false)),
            other => Err(format!("string \"{other}\" is not a valid boolean")),
        },
        // array: accept JSON array; also accept a string that JSON-parses
        //        to an array (this is how GET query-string params reach
        //        us — `?xs=[1,2,3]`).
        (ParamType::Array, Value::Array(a)) => Ok(Value::Array(a)),
        (ParamType::Array, Value::String(s)) => match serde_json::from_str::<Value>(&s) {
            Ok(Value::Array(a)) => Ok(Value::Array(a)),
            _ => Err(format!("string \"{s}\" is not a valid JSON array")),
        },
        // object: accept JSON object; string that JSON-parses to object.
        (ParamType::Object, Value::Object(o)) => Ok(Value::Object(o)),
        (ParamType::Object, Value::String(s)) => match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(o)) => Ok(Value::Object(o)),
            _ => Err(format!("string \"{s}\" is not a valid JSON object")),
        },
        // Anything else is a family mismatch.
        (_, other) => Err(json_type_name(&other).to_string()),
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn sql_err(e: sqlx::Error) -> ResqlError {
    ResqlError::SqlExecution(e.to_string())
}

async fn finalise_tx<'c>(
    tx: sqlx::Transaction<'c, sqlx::Postgres>,
    result: Result<Vec<Map<String, Value>>, ResqlError>,
) -> Result<Vec<Map<String, Value>>, ResqlError> {
    match result {
        Ok(rows) => {
            tx.commit().await.map_err(sql_err)?;
            Ok(rows)
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

async fn finalise_tx_sqlite<'c>(
    tx: sqlx::Transaction<'c, sqlx::Sqlite>,
    result: Result<Vec<Map<String, Value>>, ResqlError>,
) -> Result<Vec<Map<String, Value>>, ResqlError> {
    match result {
        Ok(rows) => {
            tx.commit().await.map_err(sql_err)?;
            Ok(rows)
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

async fn run_pg<'e, E>(
    exec: E,
    sql: &str,
    names: &[String],
    params: &Map<String, Value>,
) -> Result<Vec<Map<String, Value>>, ResqlError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let mut q = sqlx::query(sql);
    for name in names {
        let v = params.get(name).cloned().unwrap_or(Value::Null);
        q = bind_pg(q, v);
    }
    let rows = q.fetch_all(exec).await.map_err(sql_err)?;
    Ok(rows.iter().map(pg_row_to_json).collect())
}

async fn run_sqlite<'e, E>(
    exec: E,
    sql: &str,
    names: &[String],
    params: &Map<String, Value>,
) -> Result<Vec<Map<String, Value>>, ResqlError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let mut q = sqlx::query(sql);
    for name in names {
        let v = params.get(name).cloned().unwrap_or(Value::Null);
        q = bind_sqlite(q, v);
    }
    let rows = q.fetch_all(exec).await.map_err(sql_err)?;
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
        Value::Array(items) => bind_pg_array(q, items),
        Value::Object(_) => q.bind(sqlx::types::Json(v)),
    }
}

/// Homogeneous-scalar arrays bind natively (text[], int8[], float8[],
/// bool[]) so callers can do `unnest(:xs)` in one round-trip. Anything
/// else (empty, all-null, mixed, nested) falls back to JSONB.
///
/// Nulls inside an otherwise-homogeneous array stay as SQL NULL elements
/// (via `Vec<Option<T>>`).
fn bind_pg_array<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    items: Vec<Value>,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match detect_pg_array_kind(&items) {
        Some(PgArrayKind::Int) => {
            let v: Vec<Option<i64>> = items
                .iter()
                .map(|e| match e {
                    Value::Null => None,
                    Value::Number(n) => n.as_i64().or_else(|| n.as_u64().map(|u| u as i64)),
                    _ => None,
                })
                .collect();
            q.bind(v)
        }
        Some(PgArrayKind::Float) => {
            let v: Vec<Option<f64>> = items
                .iter()
                .map(|e| match e {
                    Value::Null => None,
                    Value::Number(n) => n.as_f64(),
                    _ => None,
                })
                .collect();
            q.bind(v)
        }
        Some(PgArrayKind::Text) => {
            let v: Vec<Option<String>> = items
                .iter()
                .map(|e| match e {
                    Value::Null => None,
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            q.bind(v)
        }
        Some(PgArrayKind::Bool) => {
            let v: Vec<Option<bool>> = items
                .iter()
                .map(|e| match e {
                    Value::Null => None,
                    Value::Bool(b) => Some(*b),
                    _ => None,
                })
                .collect();
            q.bind(v)
        }
        None => {
            // Empty, all-null, mixed, or nested — bind as JSONB and let
            // the SQL author use jsonb_array_elements* if they need it.
            // Emit a debug-level trace so operators can see when the
            // heuristic couldn't infer a scalar type; not warn because
            // legitimate JSONB use is common.
            tracing::debug!("array parameter has no homogeneous scalar type; binding as JSONB");
            q.bind(sqlx::types::Json(Value::Array(items)))
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PgArrayKind {
    Int,
    Float,
    Text,
    Bool,
}

fn detect_pg_array_kind(elems: &[Value]) -> Option<PgArrayKind> {
    let mut kind: Option<PgArrayKind> = None;
    let mut saw_non_null = false;
    for e in elems {
        let observed = match e {
            Value::Null => continue,
            Value::Bool(_) => PgArrayKind::Bool,
            Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    PgArrayKind::Int
                } else {
                    PgArrayKind::Float
                }
            }
            Value::String(_) => PgArrayKind::Text,
            Value::Array(_) | Value::Object(_) => return None,
        };
        saw_non_null = true;
        kind = Some(match (kind, observed) {
            (None, k) => k,
            (Some(existing), k) if existing == k => existing,
            // Int + Float → promote to Float (Postgres float8[] holds both).
            (Some(PgArrayKind::Int), PgArrayKind::Float)
            | (Some(PgArrayKind::Float), PgArrayKind::Int) => PgArrayKind::Float,
            _ => return None, // mixed scalar kinds
        });
    }
    if saw_non_null {
        kind
    } else {
        None
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
    // Timestamps and dates: stringify as ISO 8601. `NaiveDateTime`'s Display
    // uses a space separator (`2026-01-01 10:20:30`), which is neither ISO
    // 8601 nor RFC 3339 and breaks JSON consumers; force the `T` separator.
    // For TIMESTAMPTZ we emit the trailing `Z` form (Jackson / JVM Resql
    // default) rather than chrono's `+00:00`.
    if matches!(
        ty_upper.as_str(),
        "TIMESTAMP" | "TIMESTAMPTZ" | "DATE" | "TIME" | "TIMETZ"
    ) {
        if let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveDateTime>, _>(idx) {
            return Value::String(v.format("%Y-%m-%dT%H:%M:%S%.f").to_string());
        }
        if let Ok(Some(v)) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(idx) {
            return Value::String(v.format("%Y-%m-%dT%H:%M:%S%.fZ").to_string());
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

    /// Build a permissive declaration for a set of named params, all
    /// `string`, all optional. Used by tests that want to exercise the
    /// execute path without repeating the fence in every fixture.
    fn declare(params: &[&str]) -> Declaration {
        let mut yaml = String::from("params:\n");
        if params.is_empty() {
            yaml.push_str("  {}\n");
        } else {
            for n in params {
                yaml.push_str(&format!("  {n}: {{ type: string, required: false }}\n"));
            }
        }
        serde_yaml_ng::from_str(&yaml).unwrap()
    }

    #[tokio::test]
    async fn missing_optional_param_binds_null() {
        // The regression for issue #4: SQL references :missing_key, the
        // caller doesn't send it, and because it's optional the bind
        // layer sees NULL — not a MissingParameter error.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let rows = execute(
            &Pool::Sqlite(pool),
            "SELECT :missing_key IS NULL AS is_null",
            &declare(&["missing_key"]),
            &json!({}),
        )
        .await
        .unwrap();
        assert_eq!(rows[0].get("isNull").unwrap(), &json!(1));
    }

    #[tokio::test]
    async fn required_param_missing_errors() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let decl: Declaration =
            serde_yaml_ng::from_str("params:\n  needed: { type: string, required: true }\n")
                .unwrap();
        let err = execute(
            &Pool::Sqlite(pool),
            "SELECT :needed AS x",
            &decl,
            &json!({}),
        )
        .await
        .unwrap_err();
        match err {
            ResqlError::MissingParameter(name) => assert_eq!(name, "needed"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_param_rejected() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let err = execute(
            &Pool::Sqlite(pool),
            "SELECT :used AS u",
            &declare(&["used"]),
            &json!({"used": "ok", "unused": "x"}),
        )
        .await
        .unwrap_err();
        match err {
            ResqlError::UnknownParameter(name) => assert_eq!(name, "unused"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wrong_type_rejected() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let decl: Declaration =
            serde_yaml_ng::from_str("params:\n  n: { type: integer }\n").unwrap();
        let err = execute(
            &Pool::Sqlite(pool),
            "SELECT :n AS n",
            &decl,
            &json!({"n": true}),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, ResqlError::InvalidParameterType { .. }),
            "err = {err:?}"
        );
    }

    #[tokio::test]
    async fn default_used_when_optional_absent() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let decl: Declaration = serde_yaml_ng::from_str(
            "params:\n  s: { type: string, required: false, default: fallback }\n",
        )
        .unwrap();
        let rows = execute(&Pool::Sqlite(pool), "SELECT :s AS s", &decl, &json!({}))
            .await
            .unwrap();
        assert_eq!(rows[0].get("s").unwrap(), &json!("fallback"));
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
            &declare(&[]),
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
            &declare(&[]),
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
            &declare(&["v"]),
            &json!({"v": Value::Null}),
        )
        .await
        .unwrap();
        assert_eq!(rows[0].get("isNull").unwrap(), &json!(1));
    }

    #[test]
    fn array_kind_detects_int() {
        let v = vec![json!(1), json!(2), json!(-3)];
        assert_eq!(detect_pg_array_kind(&v), Some(PgArrayKind::Int));
    }

    #[test]
    fn array_kind_detects_text() {
        let v = vec![json!("a"), json!("b")];
        assert_eq!(detect_pg_array_kind(&v), Some(PgArrayKind::Text));
    }

    #[test]
    fn array_kind_detects_bool() {
        let v = vec![json!(true), json!(false)];
        assert_eq!(detect_pg_array_kind(&v), Some(PgArrayKind::Bool));
    }

    #[test]
    fn array_kind_promotes_int_to_float_when_mixed() {
        let v = vec![json!(1), json!(2.5), json!(3)];
        assert_eq!(detect_pg_array_kind(&v), Some(PgArrayKind::Float));
    }

    #[test]
    fn array_kind_permits_null_gaps() {
        let v = vec![json!(1), json!(Value::Null), json!(3)];
        assert_eq!(detect_pg_array_kind(&v), Some(PgArrayKind::Int));
    }

    #[test]
    fn array_kind_none_when_mixed_scalars() {
        let v = vec![json!(1), json!("two"), json!(3)];
        assert_eq!(detect_pg_array_kind(&v), None);
    }

    #[test]
    fn array_kind_none_when_nested() {
        let v = vec![json!([1, 2]), json!([3, 4])];
        assert_eq!(detect_pg_array_kind(&v), None);
    }

    #[test]
    fn array_kind_none_when_empty() {
        let v: Vec<Value> = vec![];
        assert_eq!(detect_pg_array_kind(&v), None);
    }

    #[test]
    fn array_kind_none_when_all_null() {
        let v = vec![json!(Value::Null), json!(Value::Null)];
        assert_eq!(detect_pg_array_kind(&v), None);
    }

    #[tokio::test]
    async fn body_must_be_object_or_null() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let err = execute(
            &Pool::Sqlite(pool),
            "SELECT 1",
            &declare(&[]),
            &json!("not an object"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ResqlError::MalformedRequest(_)));
    }

    #[test]
    fn coerce_integer_from_number_string_and_wholef64() {
        assert_eq!(coerce_to(ParamType::Integer, json!(42)).unwrap(), json!(42));
        assert_eq!(
            coerce_to(ParamType::Integer, json!("42")).unwrap(),
            json!(42)
        );
        assert_eq!(
            coerce_to(ParamType::Integer, json!(42.0)).unwrap(),
            json!(42)
        );
        assert!(coerce_to(ParamType::Integer, json!(42.5)).is_err());
        assert!(coerce_to(ParamType::Integer, json!("nope")).is_err());
    }

    #[test]
    fn coerce_boolean_from_string_forms() {
        assert_eq!(
            coerce_to(ParamType::Boolean, json!("true")).unwrap(),
            json!(true)
        );
        assert_eq!(
            coerce_to(ParamType::Boolean, json!("0")).unwrap(),
            json!(false)
        );
        assert!(coerce_to(ParamType::Boolean, json!("yes")).is_err());
    }

    #[test]
    fn coerce_number_from_integer_and_string() {
        assert!(matches!(
            coerce_to(ParamType::Number, json!(1)).unwrap(),
            Value::Number(_)
        ));
        assert!(matches!(
            coerce_to(ParamType::Number, json!("3.14")).unwrap(),
            Value::Number(_)
        ));
        assert!(coerce_to(ParamType::Number, json!("nope")).is_err());
    }

    #[test]
    fn coerce_array_from_json_string() {
        assert_eq!(
            coerce_to(ParamType::Array, json!("[1,2,3]")).unwrap(),
            json!([1, 2, 3])
        );
        assert!(coerce_to(ParamType::Array, json!("not-json")).is_err());
    }

    #[test]
    fn validate_rejects_required_null() {
        let decl: Declaration =
            serde_yaml_ng::from_str("params:\n  n: { type: string, required: true }\n").unwrap();
        let mut incoming = Map::new();
        incoming.insert("n".to_string(), Value::Null);
        let err = validate_params(&decl, incoming).unwrap_err();
        assert!(matches!(err, ResqlError::MissingParameter(_)));
    }

    #[test]
    fn validate_fills_missing_optional_with_default() {
        let decl: Declaration = serde_yaml_ng::from_str(
            "params:\n  n: { type: integer, required: false, default: 7 }\n",
        )
        .unwrap();
        let out = validate_params(&decl, Map::new()).unwrap();
        assert_eq!(out.get("n").unwrap(), &json!(7));
    }

    #[test]
    fn validate_accepts_value_in_enum() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  status: { type: string, required: true, enum: [active, disabled] }\n*/\nSELECT :status;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("status".to_string(), json!("active"));
        let out = validate_params(&decl, incoming).unwrap();
        assert_eq!(out["status"], json!("active"));
    }

    #[test]
    fn validate_rejects_value_outside_enum() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  status: { type: string, required: true, enum: [active, disabled] }\n*/\nSELECT :status;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("status".to_string(), json!("pending"));
        let err = validate_params(&decl, incoming).unwrap_err();
        match err {
            ResqlError::InvalidParameterValue {
                name,
                value,
                allowed,
            } => {
                assert_eq!(name, "status");
                assert_eq!(value, "pending");
                assert_eq!(allowed, vec!["active".to_string(), "disabled".to_string()]);
            }
            other => panic!("expected InvalidParameterValue, got {other:?}"),
        }
    }

    #[test]
    fn validate_null_bypasses_enum_check() {
        // Optional param, explicit null in request. Null bypasses enum
        // (JSON Schema semantics — required/optional gate handles
        // nullability separately).
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  status: { type: string, enum: [active, disabled] }\n*/\nSELECT :status;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("status".to_string(), Value::Null);
        let out = validate_params(&decl, incoming).unwrap();
        assert!(out["status"].is_null());
    }

    #[test]
    fn validate_missing_optional_with_enum_binds_null() {
        // Same as above but omitted from the request rather than
        // explicitly null.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  status: { type: string, enum: [active, disabled] }\n*/\nSELECT :status;\n",
        )
        .unwrap();
        let out = validate_params(&decl, Map::new()).unwrap();
        assert!(out["status"].is_null());
    }

    #[test]
    fn validate_get_string_value_is_enum_checked_after_coercion() {
        // GET query strings arrive as strings; the coercion pass keeps
        // strings as-is, and the enum check runs against the coerced
        // value. This test locks in that ordering.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  status: { type: string, required: true, enum: [active, disabled] }\n*/\nSELECT :status;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("status".to_string(), json!("bogus"));
        assert!(matches!(
            validate_params(&decl, incoming).unwrap_err(),
            ResqlError::InvalidParameterValue { .. }
        ));
    }
}
