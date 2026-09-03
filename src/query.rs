use serde_json::{Map, Value};
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
use sqlx::{Column, Row, TypeInfo};

use crate::db::Pool;
use crate::declaration::{Declaration, DeclaredParam, ItemType, ParamType};
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
        let b = bytes[i];

        // Non-ASCII byte — part of a multi-byte UTF-8 sequence. All the
        // delimiters this parser looks for (`:`, `'`, `"`, `-`, `/`, `*`)
        // are ASCII, so a non-ASCII leading byte can never start one of
        // them; copy the whole character through verbatim rather than
        // truncating it to its first byte via `as char` (see
        // `copy_utf8_char` for why the naive per-byte cast corrupts
        // multi-byte characters).
        if !b.is_ascii() {
            i = copy_utf8_char(sql, bytes, i, n, &mut out);
            continue;
        }

        let c = b as char;

        // Line comment
        if c == '-' && i + 1 < n && bytes[i + 1] == b'-' {
            while i < n && bytes[i] != b'\n' {
                if !bytes[i].is_ascii() {
                    i = copy_utf8_char(sql, bytes, i, n, &mut out);
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            continue;
        }
        // Block comment
        if c == '/' && i + 1 < n && bytes[i + 1] == b'*' {
            out.push_str("/*");
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                if !bytes[i].is_ascii() {
                    i = copy_utf8_char(sql, bytes, i, n, &mut out);
                } else {
                    out.push(bytes[i] as char);
                    i += 1;
                }
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
                if !bytes[i].is_ascii() {
                    i = copy_utf8_char(sql, bytes, i, n, &mut out);
                    continue;
                }
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

/// Copy one complete UTF-8 character starting at `sql[pos..]` into `out`,
/// returning the byte position just past it. The leading byte's high bits
/// determine the sequence length per RFC 3629 (2/3/4 bytes); this parser
/// never needs to inspect the character's value, only preserve it intact,
/// so no decoding beyond "how many bytes does this sequence span" is done.
fn copy_utf8_char(sql: &str, bytes: &[u8], pos: usize, len: usize, out: &mut String) -> usize {
    let seq_len = match bytes[pos] & 0xF0 {
        0xC0 | 0xD0 => 2,
        0xE0 => 3,
        _ => 4,
    };
    let end = (pos + seq_len).min(len);
    out.push_str(&sql[pos..end]);
    end
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
        Pool::Postgres(pg) => run_pg(pg, &rewritten, &param_names, &param_map, declaration).await,
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
            let result = run_pg(&mut *tx, &rewritten, &param_names, &param_map, declaration).await;
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
                match run_pg(&mut *tx, &rewritten, &param_names, pm, declaration).await {
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
            // Strict format check for scalar semantic types (uuid /
            // date / datetime) — mirrors the per-element check in
            // `coerce_element`. Wire binding for scalars stays text
            // (Postgres implicit-casts on assignment), so this
            // doesn't change any OID; it moves the error surface for
            // bad literals from a Postgres `BadSqlGrammarException`
            // to a request-boundary `InvalidParameterType` naming
            // the offending value. Unified with the array-element
            // path so the declaration is the strict contract at
            // every level.
            validate_semantic_format(spec.ty, name, &coerced)?;
            // Per-element coercion for arrays with declared `items.type`.
            // Without this, wrong-typed elements pass validation and get
            // shoved into a native Postgres array of the *element's* JSON
            // type — an insert into a `text[]` column with `items: {type:
            // string}` but numeric elements would silently succeed as an
            // int8[] wire bind (or worse, fall back to JSONB for empty /
            // all-null arrays and mismatch a `text[]` column). Reject at
            // the boundary so the declaration is the single source of truth
            // for the array's element type (issue #11).
            let coerced = coerce_array_items(spec, name, coerced)?;
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

/// Boot-time check: every declared `default:` must survive the same
/// coerce pipeline the runtime path applies to caller-supplied values.
///
/// Without this, a misdeclared default (`n: {type: integer, default:
/// "not-a-number"}` or `xs: {type: array, items: {type: string},
/// default: [1, 2]}`) sits latent — the endpoint works for every call
/// that supplies the param and blows up (or worse, silently drops
/// elements) only when the caller happens to omit it. Fail fast at
/// load so the shape of the world at boot matches the shape at
/// request time.
///
/// Runs after `declaration::parse`; keeps the existing enum/default
/// cross-check in `declaration::validate_enum_shapes` separate — that
/// one runs *inside* parse because it needs no runtime coercion.
pub fn validate_declaration_defaults(decl: &Declaration) -> Result<(), String> {
    for (name, spec) in &decl.params {
        let Some(default) = &spec.default else {
            continue;
        };
        if default.is_null() {
            continue;
        }
        let coerced = coerce_to(spec.ty, default.clone()).map_err(|actual| {
            format!(
                "param `{name}`: default {default} does not match declared \
                 type `{}` (got {actual})",
                spec.ty.as_str()
            )
        })?;
        // Scalar semantic-format check for the default — same helper
        // the runtime path applies to caller-supplied scalars, so
        // `default: "not-a-uuid"` on `type: uuid` fails at load
        // instead of only when a caller happens to omit the param.
        validate_semantic_format(spec.ty, name, &coerced).map_err(|e| match e {
            ResqlError::InvalidParameterType {
                name: _,
                expected,
                actual,
            } => {
                format!("param `{name}`: default {default} is not a valid `{expected}` ({actual})")
            }
            other => format!("param `{name}`: default {default} is invalid: {other}"),
        })?;
        // For arrays with declared items, catch wrong-typed elements
        // in the default at boot instead of on the first
        // default-fallback request.
        coerce_array_items(spec, name, coerced).map_err(|e| match e {
            ResqlError::InvalidParameterType {
                name: elem_name,
                expected,
                actual,
            } => format!(
                "param `{name}`: default {default} has element `{elem_name}` \
                 of wrong type (expected `{expected}`, got {actual})"
            ),
            other => format!("param `{name}`: default {default} is invalid: {other}"),
        })?;
    }
    Ok(())
}

/// If `spec` is `type: array` with an `items:` block, coerce each
/// element to the declared element type. Nulls pass (bind as SQL NULL
/// elements — declaring per-element required is not modeled here).
/// A non-array value or a spec with no `items` is returned unchanged;
/// scalar/type-family enforcement is `coerce_to`'s job.
///
/// Rejects with `InvalidParameterType` naming the failing element by
/// index so authors can pinpoint bad payloads. The reason this happens
/// here rather than in `coerce_to` is that the element type only
/// exists on `DeclaredParam.items`, not on the scalar `ParamType`.
fn coerce_array_items(spec: &DeclaredParam, name: &str, value: Value) -> Result<Value, ResqlError> {
    if spec.ty != ParamType::Array {
        return Ok(value);
    }
    let Some(items_spec) = &spec.items else {
        return Ok(value);
    };
    let Value::Array(elements) = value else {
        return Ok(value);
    };
    let mut coerced = Vec::with_capacity(elements.len());
    for (idx, elem) in elements.into_iter().enumerate() {
        coerced.push(coerce_element(items_spec, &format!("{name}[{idx}]"), elem)?);
    }
    Ok(Value::Array(coerced))
}

/// Coerce and validate one array element against a declared
/// `ItemType`. Recurses into `items_spec.items` for nested arrays so
/// a mistyped grand-child element bubbles up as `xs[0][1]` instead
/// of silently being shoved into JSONB (declaring nested items is
/// the only reason to reach for `items: {type: array, items: ...}`
/// — Postgres arrays are physically flat, so the wire binding is
/// JSONB either way, but the validation path is now recursive).
///
/// Beyond the family check that `coerce_to` performs, this also
/// runs strict format validation for the semantic string types
/// (`uuid`, `date`, `datetime`). Native `uuid[]` / `date[]` /
/// `timestamptz[]` binding downstream depends on every element
/// parsing cleanly; catching bad formats here yields an
/// `InvalidParameterType` at the request boundary with the failing
/// path, instead of a cryptic `Encode` error at bind time.
fn coerce_element(items_spec: &ItemType, path: &str, value: Value) -> Result<Value, ResqlError> {
    let coerced =
        coerce_to(items_spec.ty, value).map_err(|actual| ResqlError::InvalidParameterType {
            name: path.to_string(),
            expected: items_spec.ty.as_str(),
            actual,
        })?;
    validate_semantic_format(items_spec.ty, path, &coerced)?;
    // Recurse into nested arrays. Only meaningful when the element
    // type is `array` AND a nested `items` block was declared —
    // without the inner spec there's nothing to validate against.
    if items_spec.ty == ParamType::Array {
        if let Some(nested) = &items_spec.items {
            if let Value::Array(inner) = coerced {
                let mut out = Vec::with_capacity(inner.len());
                for (idx, e) in inner.into_iter().enumerate() {
                    out.push(coerce_element(nested, &format!("{path}[{idx}]"), e)?);
                }
                return Ok(Value::Array(out));
            }
        }
        return Ok(coerced);
    }
    Ok(coerced)
}

/// Strict format check for the semantic string types. Non-string
/// values (already coerced null, or non-string family) pass through
/// silently — this only bites when we know we have a string and its
/// contents must parse as the declared shape.
fn validate_semantic_format(ty: ParamType, path: &str, v: &Value) -> Result<(), ResqlError> {
    let Value::String(s) = v else {
        return Ok(());
    };
    let ok = match ty {
        ParamType::Uuid => uuid::Uuid::parse_str(s).is_ok(),
        ParamType::Date => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok(),
        ParamType::Datetime => parse_datetime(s).is_some(),
        _ => return Ok(()),
    };
    if ok {
        Ok(())
    } else {
        Err(ResqlError::InvalidParameterType {
            name: path.to_string(),
            expected: ty.as_str(),
            actual: format!("invalid {} literal `{s}`", ty.as_str()),
        })
    }
}

/// Parse a datetime literal into `DateTime<Utc>`. Accepts RFC 3339
/// with any offset (normalised to UTC) and — as a lenient fallback —
/// a naive `YYYY-MM-DD HH:MM:SS[.ffffff]` form assumed to be UTC.
/// The lenient branch matches the "sent from a client that forgot
/// its TZ" shape the reporter is likely to hit; the assumption is
/// documented (declaring `datetime` means "UTC-anchored instant" in
/// this project, matching how `pg_column_value` renders TIMESTAMPTZ).
fn parse_datetime(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                naive,
                chrono::Utc,
            ));
        }
    }
    None
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
    declaration: &Declaration,
) -> Result<Vec<Map<String, Value>>, ResqlError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let mut q = sqlx::query(sql);
    for name in names {
        let v = params.get(name).cloned().unwrap_or(Value::Null);
        // Declared type drives the wire OID. `validate_params` guarantees
        // every `name` in `names` is a declared param (SQL that references
        // an undeclared `:name` fails at boot, so the runtime never sees
        // one here) — the `String` fallback exists only to satisfy the
        // type-checker; it is unreachable at run time.
        let declared = declaration.params.get(name);
        let declared_ty = declared.map(|p| p.ty).unwrap_or(ParamType::String);
        let items_ty = declared.and_then(|p| p.items.as_ref()).map(|i| i.ty);
        q = bind_pg(q, v, declared_ty, items_ty);
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

/// Bind one JSON value into a Postgres query at the position of the next
/// unbound placeholder.
///
/// The Rust bind type is chosen by the **declared** `ParamType`, never by
/// the shape of the incoming JSON value. sqlx-postgres caches prepared
/// statements per SQL text on a connection: the first `Parse` fixes each
/// `$N` placeholder's type from the bind's OID, and later executions
/// against that same cached statement must bind the same OID or Postgres
/// interprets the wire bytes as the cached type (e.g. an `i64`'s binary
/// bytes as text → `invalid byte sequence for encoding "UTF8": 0x00`).
///
/// Binding by declared type keeps the OID invariant across every call for
/// a given endpoint: an `integer` param binds as `i8[i64]` whether the
/// caller sends `null`, `42`, or omits it (validated + defaulted upstream);
/// a `string` param binds as `text[String]`; etc. The declaration is the
/// single source of truth for wire typing.
fn bind_pg<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    v: Value,
    declared_ty: ParamType,
    items_ty: Option<ParamType>,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match declared_ty {
        ParamType::String | ParamType::Date | ParamType::Datetime | ParamType::Uuid => match v {
            Value::Null => q.bind(Option::<String>::None),
            Value::String(s) => q.bind(s),
            // Unreachable: validate_params coerces to string family.
            other => q.bind(other.to_string()),
        },
        ParamType::Integer => match v {
            Value::Null => q.bind(Option::<i64>::None),
            Value::Number(n) => q.bind(n.as_i64().unwrap_or_default()),
            // Unreachable: validate_params coerces to integer.
            _ => q.bind(Option::<i64>::None),
        },
        ParamType::Number => match v {
            Value::Null => q.bind(Option::<f64>::None),
            Value::Number(n) => q.bind(n.as_f64().unwrap_or_default()),
            _ => q.bind(Option::<f64>::None),
        },
        ParamType::Boolean => match v {
            Value::Null => q.bind(Option::<bool>::None),
            Value::Bool(b) => q.bind(b),
            _ => q.bind(Option::<bool>::None),
        },
        ParamType::Array => match v {
            Value::Null => bind_pg_null_array(q, items_ty),
            Value::Array(items) => bind_pg_array(q, items, items_ty),
            other => q.bind(sqlx::types::Json(other)),
        },
        ParamType::Object => match v {
            Value::Null => q.bind(Option::<sqlx::types::Json<Value>>::None),
            other => q.bind(sqlx::types::Json(other)),
        },
    }
}

/// Bind a NULL array. When the declaration names an element type, bind
/// as the matching native Postgres array so the wire OID stays stable
/// across null/non-null calls on the same cached statement (same OID-
/// pinning invariant as `bind_pg` for scalars — issue #9). When no
/// element type is declared, JSONB is the only safe default because
/// there's nothing to key the array type off of.
///
/// Note the semantic-string branches split from plain `String`:
/// `date` binds as `date[]`, `datetime` as `timestamptz[]`, `uuid` as
/// `uuid[]`. These OIDs must match what `bind_pg_array_typed` picks
/// for the populated path or the cached-statement slot re-Parse
/// error described in #9 surfaces as an array flavour.
fn bind_pg_null_array<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    items_ty: Option<ParamType>,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match items_ty {
        Some(ParamType::Integer) => q.bind(Option::<Vec<Option<i64>>>::None),
        Some(ParamType::Number) => q.bind(Option::<Vec<Option<f64>>>::None),
        Some(ParamType::Boolean) => q.bind(Option::<Vec<Option<bool>>>::None),
        Some(ParamType::String) => q.bind(Option::<Vec<Option<String>>>::None),
        Some(ParamType::Uuid) => q.bind(Option::<Vec<Option<uuid::Uuid>>>::None),
        Some(ParamType::Date) => q.bind(Option::<Vec<Option<chrono::NaiveDate>>>::None),
        Some(ParamType::Datetime) => {
            q.bind(Option::<Vec<Option<chrono::DateTime<chrono::Utc>>>>::None)
        }
        // Nested arrays/objects don't have a native flat-array analogue
        // in Postgres: JSONB is the honest representation.
        Some(ParamType::Array | ParamType::Object) | None => {
            q.bind(Option::<sqlx::types::Json<Value>>::None)
        }
    }
}

/// Homogeneous-scalar arrays bind natively (text[], int8[], float8[],
/// bool[]) so callers can do `unnest(:xs)` in one round-trip. Anything
/// with no inferrable element type falls back to JSONB.
///
/// Precedence: **declared `items.type` wins over the runtime heuristic.**
/// This is what makes `INSERT INTO t (col) VALUES (:xs)` into a
/// `text[]` column work with an empty request — `[]` alone gives the
/// heuristic nothing to key on, but `items: {type: string}` tells us to
/// bind an empty `text[]` (issue #11). Element-level type conformance
/// is already enforced by `coerce_array_items` at validation time, so
/// by the time we're here every non-null element matches `items_ty`.
///
/// Nulls inside an otherwise-homogeneous array stay as SQL NULL elements
/// (via `Vec<Option<T>>`).
fn bind_pg_array<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    items: Vec<Value>,
    items_ty: Option<ParamType>,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    if let Some(ty) = items_ty {
        return bind_pg_array_typed(q, items, ty);
    }
    match detect_pg_array_kind(&items) {
        Some(PgArrayKind::Int) => q.bind(collect_int_array(&items)),
        Some(PgArrayKind::Float) => q.bind(collect_float_array(&items)),
        Some(PgArrayKind::Text) => q.bind(collect_text_array(&items)),
        Some(PgArrayKind::Bool) => q.bind(collect_bool_array(&items)),
        None => {
            // Empty, all-null, mixed, or nested and no declared items.type
            // to steer by — bind as JSONB and let the SQL author use
            // jsonb_array_elements* if they need it. Debug-level trace so
            // operators can see when the heuristic couldn't infer a scalar
            // type; not warn because legitimate JSONB use is common.
            tracing::debug!(
                "array parameter has no homogeneous scalar type and no declared \
                 `items.type`; binding as JSONB"
            );
            q.bind(sqlx::types::Json(Value::Array(items)))
        }
    }
}

/// Declared-items branch of `bind_pg_array`. Every non-null element is
/// already the right JSON family (courtesy of `coerce_array_items`), so
/// the extraction is unwrap-shaped and can't lose data on the shapes
/// that matter (numeric widening from JSON string still went through
/// `coerce_to` first). Nested arrays/objects have no flat-array
/// analogue in Postgres — fall back to JSONB in that shape.
///
/// Semantic-string branches (`uuid`, `date`, `datetime`) bind as
/// their native Postgres array types — `uuid[]` / `date[]` /
/// `timestamptz[]`. This eliminates the SQL-side `::uuid[]` cast
/// callers used to need when inserting into a native-typed column
/// and matches the null-array branch's OID picks for cache
/// stability. Every element is guaranteed parseable because
/// `coerce_element` runs `validate_semantic_format` on it before
/// we ever reach this bind.
fn bind_pg_array_typed<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    items: Vec<Value>,
    items_ty: ParamType,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match items_ty {
        ParamType::Integer => q.bind(collect_int_array(&items)),
        ParamType::Number => q.bind(collect_float_array(&items)),
        ParamType::Boolean => q.bind(collect_bool_array(&items)),
        ParamType::String => q.bind(collect_text_array(&items)),
        ParamType::Uuid => q.bind(collect_uuid_array(&items)),
        ParamType::Date => q.bind(collect_date_array(&items)),
        ParamType::Datetime => q.bind(collect_datetime_array(&items)),
        ParamType::Array | ParamType::Object => q.bind(sqlx::types::Json(Value::Array(items))),
    }
}

fn collect_int_array(items: &[Value]) -> Vec<Option<i64>> {
    items
        .iter()
        .map(|e| match e {
            Value::Null => None,
            Value::Number(n) => n.as_i64().or_else(|| n.as_u64().map(|u| u as i64)),
            _ => None,
        })
        .collect()
}

fn collect_float_array(items: &[Value]) -> Vec<Option<f64>> {
    items
        .iter()
        .map(|e| match e {
            Value::Null => None,
            Value::Number(n) => n.as_f64(),
            _ => None,
        })
        .collect()
}

fn collect_text_array(items: &[Value]) -> Vec<Option<String>> {
    items
        .iter()
        .map(|e| match e {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn collect_bool_array(items: &[Value]) -> Vec<Option<bool>> {
    items
        .iter()
        .map(|e| match e {
            Value::Null => None,
            Value::Bool(b) => Some(*b),
            _ => None,
        })
        .collect()
}

/// Parse UUID elements. `validate_semantic_format` (run at coerce
/// time by `coerce_element`) has already vetted every non-null
/// string as a valid UUID, so `.ok()` here is defensively
/// unreachable — the fallback of `None` for a bad element would be
/// wrong (SQL NULL vs. the original value the caller sent), which is
/// why the up-front validation matters.
fn collect_uuid_array(items: &[Value]) -> Vec<Option<uuid::Uuid>> {
    items
        .iter()
        .map(|e| match e {
            Value::Null => None,
            Value::String(s) => uuid::Uuid::parse_str(s).ok(),
            _ => None,
        })
        .collect()
}

fn collect_date_array(items: &[Value]) -> Vec<Option<chrono::NaiveDate>> {
    items
        .iter()
        .map(|e| match e {
            Value::Null => None,
            Value::String(s) => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok(),
            _ => None,
        })
        .collect()
}

/// UTC-anchored datetime elements. `parse_datetime` accepts RFC 3339
/// with any offset (normalised to UTC) plus a naive ISO 8601 form
/// assumed to be UTC — same shape `pg_column_value` renders
/// TIMESTAMPTZ back as (`...Z`), so a round-trip through Resql
/// preserves the wall-clock time when the caller sends a Z-suffixed
/// literal.
fn collect_datetime_array(items: &[Value]) -> Vec<Option<chrono::DateTime<chrono::Utc>>> {
    items
        .iter()
        .map(|e| match e {
            Value::Null => None,
            Value::String(s) => parse_datetime(s),
            _ => None,
        })
        .collect()
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
        // Plain TIME (WITHOUT TIME ZONE): none of the DateTime/Date
        // decodes above match a bare NaiveTime column, so without this
        // fallback a non-NULL TIME value falls through to the "explicit
        // NULL detection" probe below (which also fails, since TIME
        // doesn't decode as String either) and is silently reported as
        // JSON null instead of erroring or returning the actual value.
        if let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveTime>, _>(idx) {
            return Value::String(v.to_string());
        }
    }
    // UUID
    if ty_upper == "UUID" {
        if let Ok(Some(v)) = row.try_get::<Option<uuid::Uuid>, _>(idx) {
            return Value::String(v.to_string());
        }
    }
    // Text-family array — best-effort as a string list. Covers every
    // Postgres character type's array form, not just TEXT[]: VARCHAR[],
    // BPCHAR[]/CHAR[], NAME[], and CITEXT[] all decode into Rust as
    // Vec<String> via sqlx, but were previously left to fall through to
    // the (always-failing, for arrays) Vec<i64> attempt below and come
    // back as null.
    if ty_upper.starts_with("_TEXT")
        || ty_upper.starts_with("_VARCHAR")
        || ty_upper.starts_with("_BPCHAR")
        || ty_upper.starts_with("_NAME")
        || ty_upper.starts_with("_CITEXT")
        || ty_upper == "TEXT[]"
        || ty_upper == "VARCHAR[]"
        || ty_upper == "CHAR[]"
        || ty_upper == "BPCHAR[]"
        || ty_upper == "NAME[]"
        || ty_upper == "CITEXT[]"
    {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<String>>, _>(idx) {
            return Value::Array(v.into_iter().map(Value::String).collect());
        }
    }
    // Integer-family arrays: sqlx requires the Rust element width to
    // exactly match the Postgres array's element width (int2[] only
    // decodes as Vec<i16>, int4[] only as Vec<i32>, int8[] only as
    // Vec<i64>) — a single `Vec<i64>` attempt only ever matched int8[],
    // silently returning null for smallint[]/integer[] columns (the
    // `try_get` fails and every branch above and below it also fails to
    // decode a non-text array as a String). Try each width in turn and
    // widen the result to i64 for JSON, which has no i16/i32/i64
    // distinction.
    if ty_upper.starts_with("_INT2") || ty_upper == "INT2[]" || ty_upper == "SMALLINT[]" {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<i16>>, _>(idx) {
            return Value::Array(
                v.into_iter()
                    .map(|i| Value::Number(i64::from(i).into()))
                    .collect(),
            );
        }
    }
    if ty_upper.starts_with("_INT4")
        || ty_upper == "INT4[]"
        || ty_upper == "INT[]"
        || ty_upper == "INTEGER[]"
    {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<i32>>, _>(idx) {
            return Value::Array(
                v.into_iter()
                    .map(|i| Value::Number(i64::from(i).into()))
                    .collect(),
            );
        }
    }
    if ty_upper.starts_with("_INT8") || ty_upper == "INT8[]" || ty_upper == "BIGINT[]" {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<i64>>, _>(idx) {
            return Value::Array(v.into_iter().map(|i| Value::Number(i.into())).collect());
        }
    }
    // Float-family arrays: same width-exactness rule as ints. A
    // `float8[]` column decoded via `Vec<i64>` fails silently and the
    // whole column is misreported as JSON `null`, so the specific
    // FLOAT4/FLOAT8 branches must precede the int8[] catch-all below
    // (audit follow-up on issue #11 tests).
    if ty_upper.starts_with("_FLOAT4") || ty_upper == "FLOAT4[]" || ty_upper == "REAL[]" {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<f32>>, _>(idx) {
            return Value::Array(
                v.into_iter()
                    .map(|f| {
                        serde_json::Number::from_f64(f as f64)
                            .map(Value::Number)
                            .unwrap_or(Value::Null)
                    })
                    .collect(),
            );
        }
    }
    if ty_upper.starts_with("_FLOAT8") || ty_upper == "FLOAT8[]" || ty_upper == "DOUBLE PRECISION[]"
    {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<f64>>, _>(idx) {
            return Value::Array(
                v.into_iter()
                    .map(|f| {
                        serde_json::Number::from_f64(f)
                            .map(Value::Number)
                            .unwrap_or(Value::Null)
                    })
                    .collect(),
            );
        }
    }
    // Boolean arrays: sqlx decodes `bool[]` as `Vec<bool>`. Same
    // silent-null failure mode as float arrays before this branch
    // existed.
    if ty_upper.starts_with("_BOOL") || ty_upper == "BOOL[]" || ty_upper == "BOOLEAN[]" {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<bool>>, _>(idx) {
            return Value::Array(v.into_iter().map(Value::Bool).collect());
        }
    }
    // UUID / DATE / TIMESTAMP / TIMESTAMPTZ / TIME arrays. Each
    // mirrors the scalar decode above (Uuid::to_string, ISO 8601
    // dates, `...Z` suffix for TIMESTAMPTZ, `T` separator for naive
    // TIMESTAMP). Without these branches a `uuid[]` column decoded
    // as `null` — same silent-loss failure mode as float8[]/bool[]
    // before their fixes.
    if ty_upper.starts_with("_UUID") || ty_upper == "UUID[]" {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<uuid::Uuid>>, _>(idx) {
            return Value::Array(
                v.into_iter()
                    .map(|u| Value::String(u.to_string()))
                    .collect(),
            );
        }
    }
    if ty_upper.starts_with("_DATE") || ty_upper == "DATE[]" {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<chrono::NaiveDate>>, _>(idx) {
            return Value::Array(
                v.into_iter()
                    .map(|d| Value::String(d.to_string()))
                    .collect(),
            );
        }
    }
    if ty_upper.starts_with("_TIMESTAMPTZ")
        || ty_upper == "TIMESTAMPTZ[]"
        || ty_upper == "TIMESTAMP WITH TIME ZONE[]"
    {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<chrono::DateTime<chrono::Utc>>>, _>(idx) {
            return Value::Array(
                v.into_iter()
                    .map(|d| Value::String(d.format("%Y-%m-%dT%H:%M:%S%.fZ").to_string()))
                    .collect(),
            );
        }
    }
    if ty_upper.starts_with("_TIMESTAMP") || ty_upper == "TIMESTAMP[]" {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<chrono::NaiveDateTime>>, _>(idx) {
            return Value::Array(
                v.into_iter()
                    .map(|d| Value::String(d.format("%Y-%m-%dT%H:%M:%S%.f").to_string()))
                    .collect(),
            );
        }
    }
    if ty_upper.starts_with("_TIME") || ty_upper == "TIME[]" || ty_upper == "TIMETZ[]" {
        if let Ok(Some(v)) = row.try_get::<Option<Vec<chrono::NaiveTime>>, _>(idx) {
            return Value::Array(
                v.into_iter()
                    .map(|t| Value::String(t.to_string()))
                    .collect(),
            );
        }
    }
    // Last-chance int8[] catch-all: an anonymous array column whose
    // pgtype name we don't recognise (e.g. `SELECT ARRAY[1,2,3]`
    // without a target type) — the exact-width rule still applies,
    // so sqlx will only decode `Vec<i64>` when the array really is
    // `int8[]`. Everything else falls through to the NULL detection
    // and last-resort text branches below.
    if ty_upper.ends_with("[]") {
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
    fn rewrite_preserves_non_ascii_in_line_comment() {
        // Multi-byte UTF-8 (accented Latin, Cyrillic, ...) in a line
        // comment must pass through byte-for-byte, not get truncated to
        // its leading byte by a per-byte `as char` cast.
        let (sql, params) = rewrite_named_params("-- Описание: Ä\nSELECT :x", Dialect::Postgres);
        assert_eq!(sql, "-- Описание: Ä\nSELECT $1");
        assert_eq!(params, vec!["x"]);
    }

    #[test]
    fn rewrite_preserves_non_ascii_in_block_comment() {
        let (sql, params) = rewrite_named_params("/* kirjeldus: Ä */ SELECT :x", Dialect::Postgres);
        assert_eq!(sql, "/* kirjeldus: Ä */ SELECT $1");
        assert_eq!(params, vec!["x"]);
    }

    #[test]
    fn rewrite_preserves_non_ascii_in_string_literal() {
        // U+00C4 (Ä) encodes as the two bytes 0xC3 0x84. A byte-by-byte
        // `as char` cast on 0xC3 produces U+00C3 (Ã), then 0x84 becomes a
        // stray C1 control character — silently corrupting the literal
        // instead of erroring, which is worse.
        let (sql, _) =
            rewrite_named_params("SELECT COALESCE(:x, 'ÄRALANGEMISENI')", Dialect::Postgres);
        assert_eq!(sql, "SELECT COALESCE($1, 'ÄRALANGEMISENI')");
        let idx = sql.find('Ä').expect("Ä must survive intact");
        assert_eq!(&sql.as_bytes()[idx..idx + 2], &[0xC3, 0x84]);
    }

    #[test]
    fn rewrite_preserves_non_ascii_in_bare_sql() {
        // Non-ASCII outside any of the special contexts above (e.g. a
        // literal used as a column alias) must also survive.
        let (sql, _) = rewrite_named_params("SELECT :x AS nimi_üürikas", Dialect::Postgres);
        assert!(sql.contains("nimi_üürikas"));
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

    // ─── Issue #11: declared `items.type` drives element validation
    // and binding for arrays. Whole-cloth "why should this even pass"
    // rejection at the boundary, not silent coercion at the seam. ────

    #[test]
    fn validate_rejects_array_element_of_wrong_type() {
        // Declared `items.type: string`, elements are numbers →
        // InvalidParameterType naming the offending element index.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: string } }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("xs".to_string(), json!(["a", 2, "c"]));
        match validate_params(&decl, incoming).unwrap_err() {
            ResqlError::InvalidParameterType { name, expected, .. } => {
                assert_eq!(name, "xs[1]");
                assert_eq!(expected, "string");
            }
            other => panic!("expected InvalidParameterType, got {other:?}"),
        }
    }

    #[test]
    fn validate_permits_null_element_in_typed_array() {
        // SQL NULL element is a legitimate bind — items.type gates the
        // element's non-null type only, not its nullability.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: integer } }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("xs".to_string(), json!([1, null, 3]));
        let out = validate_params(&decl, incoming).unwrap();
        assert_eq!(out["xs"], json!([1, null, 3]));
    }

    #[test]
    fn validate_coerces_string_elements_to_declared_integer() {
        // Element-level coercion piggybacks on `coerce_to`, so string
        // "2" for items.type: integer becomes numeric 2 — mirrors the
        // scalar behaviour for GET query strings that funnel numbers as
        // strings.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: integer } }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("xs".to_string(), json!(["1", "2", "3"]));
        let out = validate_params(&decl, incoming).unwrap();
        assert_eq!(out["xs"], json!([1, 2, 3]));
    }

    #[test]
    fn validate_accepts_empty_typed_array() {
        // The reporter's case: declared items type, empty array. Must
        // survive validation; the bind layer relies on items.type to
        // pick a native array OID (not JSONB fallback).
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: string } }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("xs".to_string(), json!([]));
        let out = validate_params(&decl, incoming).unwrap();
        assert_eq!(out["xs"], json!([]));
    }

    // Corner 5: strict format validation for semantic string element
    // types. Bad UUID/date/datetime literals rejected at the request
    // boundary with the failing element path so the native uuid[] /
    // date[] / timestamptz[] bind downstream can't hit a parse error
    // it can't recover from cleanly.

    // Scalar semantic-format validation. Mirrors the per-element
    // check now applied to arrays — same helper, same error surface
    // (`InvalidParameterType`) — but the wire binding stays text so
    // no OID changes. Bad literals fail at the request boundary
    // instead of surfacing as a Postgres `BadSqlGrammarException`.

    #[test]
    fn validate_rejects_bad_scalar_uuid() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  id: { type: uuid, required: true }\n*/\nSELECT :id;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("id".to_string(), json!("not-a-uuid"));
        match validate_params(&decl, incoming).unwrap_err() {
            ResqlError::InvalidParameterType { name, expected, .. } => {
                assert_eq!(name, "id");
                assert_eq!(expected, "uuid");
            }
            other => panic!("expected InvalidParameterType, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_valid_scalar_uuid() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  id: { type: uuid, required: true }\n*/\nSELECT :id;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert(
            "id".to_string(),
            json!("11111111-1111-1111-1111-111111111111"),
        );
        validate_params(&decl, incoming).unwrap();
    }

    #[test]
    fn validate_null_scalar_uuid_bypasses_format_check() {
        // Optional param + null value must survive — format check
        // only fires for string values (mirrors the array-element
        // behaviour where null elements pass).
        let (decl, _) =
            crate::declaration::parse("/*\nparams:\n  id: { type: uuid }\n*/\nSELECT :id;\n")
                .unwrap();
        let mut incoming = Map::new();
        incoming.insert("id".to_string(), Value::Null);
        validate_params(&decl, incoming).unwrap();
    }

    #[test]
    fn validate_rejects_bad_scalar_date() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  d: { type: date, required: true }\n*/\nSELECT :d;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("d".to_string(), json!("2026-13-45"));
        match validate_params(&decl, incoming).unwrap_err() {
            ResqlError::InvalidParameterType { name, expected, .. } => {
                assert_eq!(name, "d");
                assert_eq!(expected, "date");
            }
            other => panic!("expected InvalidParameterType, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_valid_scalar_date() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  d: { type: date, required: true }\n*/\nSELECT :d;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("d".to_string(), json!("2026-07-01"));
        validate_params(&decl, incoming).unwrap();
    }

    #[test]
    fn validate_rejects_bad_scalar_datetime() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  ts: { type: datetime, required: true }\n*/\nSELECT :ts;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("ts".to_string(), json!("not-a-timestamp"));
        match validate_params(&decl, incoming).unwrap_err() {
            ResqlError::InvalidParameterType { name, expected, .. } => {
                assert_eq!(name, "ts");
                assert_eq!(expected, "datetime");
            }
            other => panic!("expected InvalidParameterType, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_valid_scalar_datetime_rfc3339() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  ts: { type: datetime, required: true }\n*/\nSELECT :ts;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("ts".to_string(), json!("2026-01-01T10:20:30Z"));
        validate_params(&decl, incoming).unwrap();
    }

    #[test]
    fn boot_rejects_scalar_default_with_bad_uuid_format() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  id: { type: uuid, default: \"not-a-uuid\" }\n*/\nSELECT :id;\n",
        )
        .unwrap();
        let err = validate_declaration_defaults(&decl).unwrap_err();
        assert!(err.contains("not a valid `uuid`"), "err = {err}");
    }

    #[test]
    fn boot_permits_scalar_default_with_valid_uuid_format() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  id: { type: uuid, default: \"11111111-1111-1111-1111-111111111111\" }\n*/\nSELECT :id;\n",
        )
        .unwrap();
        validate_declaration_defaults(&decl).unwrap();
    }

    #[test]
    fn validate_rejects_bad_uuid_element() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  ids: { type: array, items: { type: uuid } }\n*/\nSELECT :ids;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert(
            "ids".to_string(),
            json!(["11111111-1111-1111-1111-111111111111", "not-a-uuid"]),
        );
        match validate_params(&decl, incoming).unwrap_err() {
            ResqlError::InvalidParameterType { name, expected, .. } => {
                assert_eq!(name, "ids[1]");
                assert_eq!(expected, "uuid");
            }
            other => panic!("expected InvalidParameterType, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_valid_uuid_elements() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  ids: { type: array, items: { type: uuid } }\n*/\nSELECT :ids;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert(
            "ids".to_string(),
            json!([
                "11111111-1111-1111-1111-111111111111",
                "22222222-2222-2222-2222-222222222222"
            ]),
        );
        validate_params(&decl, incoming).unwrap();
    }

    #[test]
    fn validate_rejects_bad_date_element() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  ds: { type: array, items: { type: date } }\n*/\nSELECT :ds;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("ds".to_string(), json!(["2026-07-01", "yesterday"]));
        match validate_params(&decl, incoming).unwrap_err() {
            ResqlError::InvalidParameterType { name, expected, .. } => {
                assert_eq!(name, "ds[1]");
                assert_eq!(expected, "date");
            }
            other => panic!("expected InvalidParameterType, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_bad_datetime_element() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  ts: { type: array, items: { type: datetime } }\n*/\nSELECT :ts;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert(
            "ts".to_string(),
            json!(["2026-01-01T10:20:30Z", "not-a-timestamp"]),
        );
        match validate_params(&decl, incoming).unwrap_err() {
            ResqlError::InvalidParameterType { name, expected, .. } => {
                assert_eq!(name, "ts[1]");
                assert_eq!(expected, "datetime");
            }
            other => panic!("expected InvalidParameterType, got {other:?}"),
        }
    }

    #[test]
    fn parse_datetime_accepts_rfc3339_with_z() {
        assert!(parse_datetime("2026-01-01T10:20:30Z").is_some());
    }

    #[test]
    fn parse_datetime_accepts_rfc3339_with_offset() {
        let dt = parse_datetime("2026-01-01T10:20:30+02:00").unwrap();
        // Offset must be normalised to UTC (hour 8, not 10).
        assert_eq!(
            dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "2026-01-01T08:20:30Z"
        );
    }

    #[test]
    fn parse_datetime_accepts_naive_space_separator() {
        // The Postgres text output form; caller might paste it back.
        assert!(parse_datetime("2026-01-01 10:20:30").is_some());
    }

    #[test]
    fn parse_datetime_accepts_naive_t_separator() {
        assert!(parse_datetime("2026-01-01T10:20:30").is_some());
    }

    #[test]
    fn parse_datetime_rejects_garbage() {
        assert!(parse_datetime("not-a-timestamp").is_none());
        assert!(parse_datetime("").is_none());
        assert!(parse_datetime("2026-13-01T10:20:30Z").is_none());
    }

    // Corner 6: nested items support. Declared
    // `items: {type: array, items: {type: integer}}` now validates
    // grand-child elements recursively with `xs[i][j]` paths.

    #[test]
    fn validate_recurses_into_nested_typed_array() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: array, items: { type: integer } } }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("xs".to_string(), json!([[1, 2], [3, "four"]]));
        match validate_params(&decl, incoming).unwrap_err() {
            ResqlError::InvalidParameterType { name, expected, .. } => {
                assert_eq!(name, "xs[1][1]");
                assert_eq!(expected, "integer");
            }
            other => panic!("expected InvalidParameterType, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_nested_typed_array() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: array, items: { type: integer } } }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("xs".to_string(), json!([[1, 2], [3, 4]]));
        let out = validate_params(&decl, incoming).unwrap();
        assert_eq!(out["xs"], json!([[1, 2], [3, 4]]));
    }

    #[test]
    fn validate_nested_array_without_inner_items_skips_recursion() {
        // Outer items.type is array but no nested items → outer level
        // enforces "each element must be an array", inner elements
        // are not further validated (declaration didn't ask).
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: array } }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("xs".to_string(), json!([[1, "two"], [3, true]]));
        let out = validate_params(&decl, incoming).unwrap();
        assert_eq!(out["xs"], json!([[1, "two"], [3, true]]));
    }

    #[test]
    fn validate_nested_array_rejects_non_array_element() {
        // Outer items.type: array — any element that isn't an array
        // fails family check at the outer level, even without inner
        // items declared.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: array } }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        let mut incoming = Map::new();
        incoming.insert("xs".to_string(), json!([[1, 2], "not-an-array"]));
        match validate_params(&decl, incoming).unwrap_err() {
            ResqlError::InvalidParameterType { name, expected, .. } => {
                assert_eq!(name, "xs[1]");
                assert_eq!(expected, "array");
            }
            other => panic!("expected InvalidParameterType, got {other:?}"),
        }
    }

    #[test]
    fn validate_array_without_items_type_passes_elements_through() {
        // Back-compat: legacy declaration with no items.type keeps its
        // "opaque array" behaviour. The runtime bind heuristic still
        // runs.
        let (decl, _) =
            crate::declaration::parse("/*\nparams:\n  xs: { type: array }\n*/\nSELECT :xs;\n")
                .unwrap();
        let mut incoming = Map::new();
        incoming.insert("xs".to_string(), json!([1, "two", true]));
        let out = validate_params(&decl, incoming).unwrap();
        assert_eq!(out["xs"], json!([1, "two", true]));
    }

    // ─── Corner 1: defaults go through the same coerce pipeline at
    // boot so a misdeclared default can't sit latent until the caller
    // happens to omit the param. ──────────────────────────────────

    #[test]
    fn boot_rejects_scalar_default_of_wrong_family() {
        // integer param with a string default that isn't a number →
        // reject at boot, not at first default-fallback request.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  n: { type: integer, default: \"not-a-number\" }\n*/\nSELECT :n;\n",
        )
        .unwrap();
        let err = validate_declaration_defaults(&decl).unwrap_err();
        assert!(err.contains("default"), "err = {err}");
        assert!(err.contains("`integer`"), "err = {err}");
    }

    #[test]
    fn boot_permits_scalar_default_that_coerces() {
        // string-encoded integer default is fine — same coerce rule
        // as the runtime path (mirrors GET query-string arrival).
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  n: { type: integer, default: \"42\" }\n*/\nSELECT :n;\n",
        )
        .unwrap();
        validate_declaration_defaults(&decl).unwrap();
    }

    #[test]
    fn boot_rejects_array_default_with_wrong_element_type() {
        // items.type: string but default has an integer element →
        // otherwise silently becomes SQL NULL in a text[] bind.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: string }, default: [\"a\", 2] }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        let err = validate_declaration_defaults(&decl).unwrap_err();
        assert!(err.contains("xs[1]"), "err = {err}");
        assert!(err.contains("`string`"), "err = {err}");
    }

    #[test]
    fn boot_permits_array_default_matching_items() {
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: string }, default: [\"a\", \"b\"] }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        validate_declaration_defaults(&decl).unwrap();
    }

    #[test]
    fn boot_permits_empty_array_default() {
        // The reporter's `default: []` shape (implicit "no categories")
        // must load; validation only cares about element types.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: string }, default: [] }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        validate_declaration_defaults(&decl).unwrap();
    }

    #[test]
    fn boot_rejects_default_with_bad_semantic_element_format() {
        // `default: ["not-a-uuid"]` on `items: {type: uuid}` — the
        // per-element format check runs inside the boot validator
        // via `coerce_array_items` → `coerce_element` →
        // `validate_semantic_format`. Bad literals surface at load
        // rather than blowing up the first default-fallback request.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  ids: { type: array, items: { type: uuid }, default: [\"not-a-uuid\"] }\n*/\nSELECT :ids;\n",
        )
        .unwrap();
        let err = validate_declaration_defaults(&decl).unwrap_err();
        assert!(err.contains("ids[0]"), "err = {err}");
        assert!(err.contains("uuid"), "err = {err}");
    }

    #[test]
    fn boot_permits_null_default() {
        // Explicit `default: null` bypasses coercion — matches the
        // runtime enum bypass rule and the "null everywhere" opt-out.
        let (decl, _) = crate::declaration::parse(
            "/*\nparams:\n  xs: { type: array, items: { type: string }, default: null }\n*/\nSELECT :xs;\n",
        )
        .unwrap();
        validate_declaration_defaults(&decl).unwrap();
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
