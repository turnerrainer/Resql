//! Postgres-backed integration tests.
//!
//! Skips silently when `TEST_POSTGRES_URL` is unset — a developer without
//! a running Postgres still sees the SQLite suite go green. CI always
//! sets it (see `.github/workflows/tests.yml` — Postgres sidecar service
//! + Liquibase update step).
//!
//! Schema and seed data live in `db/changelog/`. Never load fixture SQL
//! inline in these tests — the whole point of Liquibase-managed schema
//! is that "this exists in the DB" is claimed by exactly one place.

mod common;

use common::TestAppBuilder;
use serde_json::{json, Value};

fn pg_url() -> Option<String> {
    std::env::var("TEST_POSTGRES_URL").ok()
}

/// Skip-with-message macro. Only usable in `#[test]`-shaped functions
/// (return type `()`). Helper functions must take the URL as an argument.
macro_rules! require_pg {
    () => {
        match pg_url() {
            Some(u) => u,
            None => {
                eprintln!(
                    "SKIP: TEST_POSTGRES_URL is unset; run `make test-pg` to include Postgres tests"
                );
                return;
            }
        }
    };
}

async fn app_with_pg(url: &str) -> common::TestApp {
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_postgres_datasource("pg", url)
        .with_sql(
            "pg/GET/users/find-by-login.sql",
            "SELECT id, login, email, status FROM users WHERE login = :login",
        )
        .with_sql(
            "pg/GET/users/list-active.sql",
            "SELECT login, email FROM users WHERE status = :status ORDER BY id",
        )
        .with_sql(
            "pg/GET/users/created-at.sql",
            "SELECT login, created_at FROM users WHERE login = :login",
        )
        .with_sql(
            "pg/GET/orders/by-user.sql",
            "SELECT id, user_id, total_cents, items FROM orders \
             WHERE user_id = (SELECT id FROM users WHERE login = :login) \
             ORDER BY id",
        )
        .with_sql(
            "pg/GET/flags/lookup.sql",
            "SELECT name, enabled FROM flags WHERE name = :name",
        )
        .with_sql(
            "pg/GET/stats/on-day.sql",
            "SELECT region, revenue FROM daily_stats WHERE day = cast(:day AS DATE) ORDER BY region",
        )
        .with_sql(
            "pg/POST/audit/append.sql",
            "INSERT INTO audit_log (actor, action) VALUES (:actor, :action) \
             RETURNING id, actor, action",
        )
        .with_sql(
            "pg/POST/users/create.sql",
            "INSERT INTO users (login, email, status) VALUES (:login, :email, :status) \
             RETURNING id, login, email, status",
        )
        .with_sql(
            "pg/POST/typechecks/pg-cast.sql",
            "/*\nparams:\n  n: { type: integer, required: true }\n*/\nSELECT cast(:n AS INTEGER) AS coerced, :n::TEXT AS as_text",
        )
        .with_sql(
            "pg/POST/typechecks/null.sql",
            "/*\nparams:\n  maybe: { type: string, required: false }\n*/\nSELECT :maybe IS NULL AS is_null, coalesce(:maybe, 'fallback') AS resolved",
        )
        .with_sql(
            "pg/POST/typechecks/required.sql",
            "/*\nparams:\n  login: { type: string, required: true }\n*/\nSELECT :login AS login",
        )
        .with_sql(
            "pg/POST/typechecks/maybe-number.sql",
            "/*\nparams:\n  maybe: { type: integer, required: false }\n*/\nSELECT :maybe IS NULL AS is_null, :maybe AS resolved",
        )
        .with_sql(
            "pg/POST/typechecks/bad-grammar.sql",
            "SELECT * FROM definitely_not_a_table",
        )
        .with_sql(
            "pg/GET/time/naked-timestamp.sql",
            "SELECT TIMESTAMP '2026-01-01 10:20:30' AS ts",
        )
        .with_sql(
            "pg/GET/time/naked-timestamptz.sql",
            "SELECT TIMESTAMP WITH TIME ZONE '2026-01-01 10:20:30+00' AS ts",
        )
        .with_sql(
            "pg/GET/time/naked-time.sql",
            "SELECT TIME '10:20:30' AS t",
        )
        .build()
        .await
}

// ─── SELECT round-trips ──────────────────────────────────────────────

#[tokio::test]
async fn pg_find_user_by_login() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("GET", "/pg/users/find-by-login?login=alice", None, &[])
        .await;
    assert_eq!(status, 200);
    let row = &body.as_array().unwrap()[0];
    assert_eq!(row["login"], "alice");
    assert_eq!(row["email"], "alice@example.com");
    assert_eq!(row["status"], "active");
    assert!(row["id"].is_number());
}

#[tokio::test]
async fn pg_list_active_users_multi_row() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("GET", "/pg/users/list-active?status=active", None, &[])
        .await;
    assert_eq!(status, 200);
    let arr = body.as_array().unwrap();
    let logins: Vec<&str> = arr.iter().map(|r| r["login"].as_str().unwrap()).collect();
    assert!(logins.contains(&"alice"));
    assert!(logins.contains(&"bob"));
    assert!(!logins.contains(&"charlie")); // disabled
}

#[tokio::test]
async fn pg_timestamptz_serialises_as_iso_z() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("GET", "/pg/users/created-at?login=alice", None, &[])
        .await;
    assert_eq!(status, 200);
    let created = body[0]["createdAt"].as_str().unwrap();
    // Liquibase seeds with NOW() so we don't know the instant — but the
    // shape must be ISO 8601 with `T` separator and trailing `Z` (issue #3,
    // matches Jackson / JVM Resql default).
    assert!(created.contains('T'), "expected T separator, got {created}");
    assert!(created.ends_with('Z'), "expected trailing Z, got {created}");
    assert!(!created.contains(' '), "no space separator: {created}");
    assert!(!created.contains('+'), "no `+HH:MM` offset: {created}");
}

#[tokio::test]
async fn pg_timestamp_literal_uses_iso_t_separator() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("GET", "/pg/time/naked-timestamp", None, &[])
        .await;
    assert_eq!(status, 200);
    // Naive TIMESTAMP: no timezone suffix, but must use `T`, not space
    // (chrono's default Display uses space — issue #3).
    assert_eq!(body[0]["ts"], "2026-01-01T10:20:30");
}

#[tokio::test]
async fn pg_timestamptz_literal_uses_iso_z_suffix() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("GET", "/pg/time/naked-timestamptz", None, &[])
        .await;
    assert_eq!(status, 200);
    // TIMESTAMPTZ at UTC: exact `...Z` form, not `+00:00` (issue #3).
    assert_eq!(body[0]["ts"], "2026-01-01T10:20:30Z");
}

/// A plain `TIME WITHOUT TIME ZONE` column matches none of the
/// `NaiveDateTime`/`DateTime<Utc>`/`NaiveDate` decode attempts in
/// `pg_column_value`'s timestamp/date branch. Without a dedicated
/// `NaiveTime` fallback, a non-NULL `TIME` value silently comes back as
/// JSON `null` instead of the actual time (the "explicit NULL detection"
/// and "last-resort text" fallbacks both also fail to decode `TIME` as
/// `String`, so the column is misreported as null rather than erroring).
#[tokio::test]
async fn pg_naked_time_column_decodes_as_string() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app.request("GET", "/pg/time/naked-time", None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["t"], "10:20:30");
}

#[tokio::test]
async fn pg_jsonb_column_comes_back_as_json_value() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("GET", "/pg/orders/by-user?login=alice", None, &[])
        .await;
    assert_eq!(status, 200);
    let arr = body.as_array().unwrap();
    assert!(!arr.is_empty(), "alice should have orders");
    for order in arr {
        assert!(
            order["items"].is_array(),
            "items must be a JSON array, got {}",
            order["items"]
        );
        let items = order["items"].as_array().unwrap();
        assert!(!items.is_empty());
        for item in items {
            assert!(item["sku"].is_string());
            assert!(item["qty"].is_number());
        }
    }
}

#[tokio::test]
async fn pg_boolean_columns_are_bool_json() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    for (name, want) in [("dark_mode", true), ("legacy_api", false)] {
        let (status, body) = app
            .request("GET", &format!("/pg/flags/lookup?name={name}"), None, &[])
            .await;
        assert_eq!(status, 200);
        assert_eq!(body[0]["enabled"], Value::Bool(want));
    }
}

#[tokio::test]
async fn pg_numeric_precision_preserved_as_string() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("GET", "/pg/stats/on-day?day=2026-07-01", None, &[])
        .await;
    assert_eq!(status, 200);
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // Postgres NUMERIC → JSON string so precision doesn't round-trip through f64.
    for row in arr {
        assert!(
            row["revenue"].is_string(),
            "revenue must be string, got {}",
            row["revenue"]
        );
    }
    let by_region: std::collections::HashMap<&str, &str> = arr
        .iter()
        .map(|r| {
            (
                r["region"].as_str().unwrap(),
                r["revenue"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(by_region["EMEA"], "12345.67");
    assert_eq!(by_region["APAC"], "9876.54");
}

// ─── INSERT / RETURNING ──────────────────────────────────────────────

#[tokio::test]
async fn pg_insert_returning_new_row() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let payload = json!({
        "login": format!("insertuser_{}", uuid()),
        "email": "ins@example.com",
        "status": "active",
    });
    let (status, body) = app
        .request("POST", "/pg/users/create", Some(&payload.to_string()), &[])
        .await;
    assert_eq!(status, 200);
    let row = &body.as_array().unwrap()[0];
    assert_eq!(row["email"], "ins@example.com");
    assert_eq!(row["status"], "active");
    assert!(row["id"].is_number());
}

#[tokio::test]
async fn pg_insert_with_null_email_ok() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let payload = json!({
        "login": format!("nullemail_{}", uuid()),
        "email": Value::Null,
        "status": "active",
    });
    let (status, body) = app
        .request("POST", "/pg/users/create", Some(&payload.to_string()), &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["email"], Value::Null);
}

#[tokio::test]
async fn pg_insert_duplicate_unique_returns_400() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let login = format!("dupetest_{}", uuid());
    let payload = json!({"login": login, "email": null, "status": "active"});
    let (s1, _) = app
        .request("POST", "/pg/users/create", Some(&payload.to_string()), &[])
        .await;
    assert_eq!(s1, 200);
    let (s2, body2) = app
        .request("POST", "/pg/users/create", Some(&payload.to_string()), &[])
        .await;
    assert_eq!(s2, 400);
    assert_eq!(body2["error"], "BadSqlGrammarException");
}

#[tokio::test]
async fn pg_audit_append_batch() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let payload = json!({
        "queries": [
            {"actor": "test-a", "action": "batch1"},
            {"actor": "test-b", "action": "batch2"},
            {"actor": "test-c", "action": "batch3"},
        ]
    });
    let (status, body) = app
        .request(
            "POST",
            "/pg/audit/append/batch",
            Some(&payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    for entry in arr {
        let inner = entry.as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert!(inner[0]["id"].is_number());
    }
}

// ─── Type coercion / NULL / casts ────────────────────────────────────

#[tokio::test]
async fn pg_cast_and_repeat_binding() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("POST", "/pg/typechecks/pg-cast", Some(r#"{"n": 42}"#), &[])
        .await;
    assert_eq!(status, 200);
    let row = &body[0];
    assert_eq!(row["coerced"], 42);
    assert_eq!(row["asText"], "42");
}

#[tokio::test]
async fn pg_null_param_is_null() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/typechecks/null",
            Some(r#"{"maybe": null}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["isNull"], true);
    assert_eq!(body[0]["resolved"], "fallback");
}

/// Guards against a connection-corrupting regression in `bind_pg`.
/// sqlx-postgres caches prepared statements per SQL text on a connection,
/// and Postgres fixes each `$N` placeholder's type on the *first* `Parse`
/// of the cached statement. An optional `integer`/`number` param that is
/// `null` on one call and a real number on a later call — both against
/// the *same* cached statement on the *same* connection — must not
/// corrupt the connection.
///
/// Previously `bind_pg` bound `Value::Null` as `Option::<String>::None`
/// (pinning the slot to text) and `Value::Number` natively as `i64/f64`
/// (binary). Later `i64` bytes going into a text-typed slot surfaced as
/// `invalid byte sequence for encoding "UTF8": 0x00` with a 400
/// `BadSqlGrammarException`. The fix binds by declared type so both
/// null and non-null bindings for the same param use the same OID.
///
/// Reproduction requires both calls to land on the same physical
/// connection. The test pool is pinned to `max_connections=1` (see
/// `tests/common/mod.rs::build`) so this reproduces deterministically —
/// with a multi-connection pool it would only reproduce probabilistically.
///
/// Diagnosis originally by @Aljoxa88 in [#7]; landed via the
/// declaration-typed-null fix, which keeps the native integer bind so
/// existing SQL like `WHERE id = :id` (no explicit cast) still works.
#[tokio::test]
async fn pg_number_after_null_on_same_cached_statement_does_not_corrupt() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;

    let (status1, body1) = app
        .request("POST", "/pg/typechecks/maybe-number", Some("{}"), &[])
        .await;
    assert_eq!(status1, 200, "first (null) call failed: {body1}");
    assert_eq!(body1[0]["isNull"], true);
    assert!(body1[0]["resolved"].is_null());

    let (status2, body2) = app
        .request(
            "POST",
            "/pg/typechecks/maybe-number",
            Some(r#"{"maybe": 42}"#),
            &[],
        )
        .await;
    assert_eq!(
        status2, 200,
        "second (number) call on the same cached statement must not \
         corrupt the connection; got {status2}: {body2}"
    );
    assert_eq!(body2[0]["isNull"], false);
    assert_eq!(body2[0]["resolved"], 42);
}

// ─── Errors ───────────────────────────────────────────────────────────

#[tokio::test]
async fn pg_missing_param_400() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("POST", "/pg/typechecks/required", Some("{}"), &[])
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "InvalidDataAccessApiUsageException");
    assert!(body["message"].as_str().unwrap().contains("'login'"));
}

#[tokio::test]
async fn pg_sql_error_400() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("POST", "/pg/typechecks/bad-grammar", Some("{}"), &[])
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "BadSqlGrammarException");
}

// ─── Column renaming ─────────────────────────────────────────────────

#[tokio::test]
async fn pg_snake_columns_become_camel() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("GET", "/pg/orders/by-user?login=alice", None, &[])
        .await;
    assert_eq!(status, 200);
    let row = &body[0];
    assert!(
        row.get("userId").is_some(),
        "expected userId (camel), got {row}"
    );
    assert!(row.get("totalCents").is_some());
    assert!(row.get("user_id").is_none(), "raw snake key must not leak");
}

// ─── Health-check-adjacent ───────────────────────────────────────────

#[tokio::test]
async fn pg_datasources_endpoint_shows_pg_driver() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app.request("GET", "/datasources", None, &[]).await;
    assert_eq!(status, 200);
    let entry = body
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "pg")
        .expect("pg datasource should be listed");
    // Java-compat: the /datasources response uses Java-canonical camelCase
    // (`driverClassName`, `jdbcUrl`) rather than short field names, so
    // existing Spring-era dashboards keep working. Assert both shape and
    // password masking.
    assert_eq!(entry["driverClassName"], "org.postgresql.Driver");
    let url_field = entry["jdbcUrl"].as_str().unwrap();
    if url_field.contains('@') {
        assert!(
            url_field.contains("*****"),
            "password should be masked, saw: {url_field}"
        );
    }
}

// ─── Task 007-3a: atomic batch ───────────────────────────────────────

/// Batch of 3 inserts where the 2nd violates UNIQUE — verify 0 rows
/// were committed. Without transactional batch, iteration 1's row would
/// persist and iteration 2 would return 400. Post-007, the whole batch
/// rolls back.
#[tokio::test]
async fn pg_batch_rolls_back_on_error() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let login_a = format!("txbatch_a_{}", uuid());
    let payload = json!({
        "queries": [
            {"login": login_a, "email": "a@x", "status": "active"},
            {"login": login_a, "email": "dup@x", "status": "active"},
            {"login": format!("txbatch_c_{}", uuid()), "email": "c@x", "status": "active"},
        ]
    });
    let (status, body) = app
        .request(
            "POST",
            "/pg/users/create/batch",
            Some(&payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "BadSqlGrammarException");
    // R9: caller-visible message identifies the failing position but
    // must not disclose the Postgres error text (which would leak
    // constraint / column / table names to the caller).
    let msg = body["message"].as_str().unwrap();
    assert!(msg.contains("2 of 3"), "msg = {msg}");
    let msg_lc = msg.to_ascii_lowercase();
    assert!(
        !msg_lc.contains("duplicate")
            && !msg_lc.contains("unique")
            && !msg_lc.contains("constraint"),
        "batch error must not leak Postgres detail: {msg}"
    );

    // Iteration 1's login must NOT be persisted.
    let (s2, found) = app
        .request(
            "GET",
            &format!("/pg/users/find-by-login?login={login_a}"),
            None,
            &[],
        )
        .await;
    assert_eq!(s2, 200);
    assert!(
        found.as_array().unwrap().is_empty(),
        "iteration 1 must have rolled back; got {found}"
    );
}

// ─── Task 007-3b: native Postgres array binding ──────────────────────

async fn app_with_arrays(url: &str) -> common::TestApp {
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_postgres_datasource("pg", url)
        .with_sql(
            "pg/POST/arrays/unnest-int.sql",
            "/*\nparams:\n  xs: { type: array, required: true, items: { type: integer } }\n*/\nSELECT unnest(:xs) AS n",
        )
        .with_sql(
            "pg/POST/arrays/unnest-text.sql",
            "/*\nparams:\n  xs: { type: array, required: true, items: { type: string } }\n*/\nSELECT unnest(:xs) AS s",
        )
        .with_sql(
            "pg/POST/arrays/unnest-bool.sql",
            "/*\nparams:\n  xs: { type: array, required: true, items: { type: boolean } }\n*/\nSELECT unnest(:xs) AS b",
        )
        .with_sql(
            "pg/POST/arrays/unnest-float.sql",
            "/*\nparams:\n  xs: { type: array, required: true, items: { type: number } }\n*/\nSELECT unnest(:xs) AS f",
        )
        .with_sql(
            "pg/POST/arrays/mixed-jsonb.sql",
            // Heterogeneous → falls back to JSONB; caller must use
            // jsonb_array_elements() and pull a specific type per element.
            "/*\nparams:\n  xs: { type: array, required: true }\n*/\nSELECT jsonb_array_length(:xs::jsonb) AS len",
        )
        .with_sql(
            "pg/POST/arrays/bulk-insert.sql",
            // Single-round-trip bulk insert: two parallel arrays fanned
            // out with unnest into a real INSERT ... SELECT.
            "/*\nparams:\n  actors:  { type: array, required: true, items: { type: string } }\n  actions: { type: array, required: true, items: { type: string } }\n*/\nINSERT INTO audit_log (actor, action) SELECT unnest(:actors), unnest(:actions) RETURNING id, actor, action",
        )
        .with_sql(
            "pg/GET/arrays/smallint-column.sql",
            "SELECT ARRAY[1,2,3]::SMALLINT[] AS xs",
        )
        .with_sql(
            "pg/GET/arrays/integer-column.sql",
            "SELECT ARRAY[10,20,30]::INTEGER[] AS xs",
        )
        .with_sql(
            "pg/GET/arrays/bigint-column.sql",
            "SELECT ARRAY[100,200,300]::BIGINT[] AS xs",
        )
        .with_sql(
            "pg/GET/arrays/varchar-column.sql",
            "SELECT ARRAY['a','b','c']::VARCHAR[] AS xs",
        )
        .with_sql(
            "pg/GET/arrays/bpchar-column.sql",
            "SELECT ARRAY['a','b']::CHAR(1)[] AS xs",
        )
        .build()
        .await
}

#[tokio::test]
async fn pg_native_int_array_via_unnest() {
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/arrays/unnest-int",
            Some(r#"{"xs": [10, 20, 30]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    let ns: Vec<i64> = arr.iter().map(|r| r["n"].as_i64().unwrap()).collect();
    assert_eq!(ns, vec![10, 20, 30]);
}

#[tokio::test]
async fn pg_native_text_array_via_unnest() {
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/arrays/unnest-text",
            Some(r#"{"xs": ["alpha", "beta", "gamma"]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    let strs: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["s"].as_str().unwrap())
        .collect();
    assert_eq!(strs, vec!["alpha", "beta", "gamma"]);
}

#[tokio::test]
async fn pg_native_bool_array_via_unnest() {
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/arrays/unnest-bool",
            Some(r#"{"xs": [true, false, true]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    let bs: Vec<bool> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["b"].as_bool().unwrap())
        .collect();
    assert_eq!(bs, vec![true, false, true]);
}

#[tokio::test]
async fn pg_native_float_array_via_unnest() {
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/arrays/unnest-float",
            Some(r#"{"xs": [1.5, 2.5, 3.5]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    let fs: Vec<f64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["f"].as_f64().unwrap())
        .collect();
    assert_eq!(fs, vec![1.5, 2.5, 3.5]);
}

#[tokio::test]
async fn pg_heterogeneous_array_falls_back_to_jsonb() {
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/arrays/mixed-jsonb",
            Some(r#"{"xs": [1, "two", true]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["len"], 3);
}

/// `pg_column_value` only ever attempted `Vec<i64>` for integer-family
/// arrays, which sqlx only decodes successfully for `int8[]` (the exact
/// Rust width must match the Postgres array's element width). A
/// `smallint[]`/`integer[]` column's `try_get::<Option<Vec<i64>>>` fails,
/// and every other branch also fails to decode a non-text array as a
/// `String`, so the column was silently misreported as JSON `null`.
#[tokio::test]
async fn pg_smallint_array_column_decodes() {
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request("GET", "/pg/arrays/smallint-column", None, &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["xs"], json!([1, 2, 3]));
}

#[tokio::test]
async fn pg_integer_array_column_decodes() {
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request("GET", "/pg/arrays/integer-column", None, &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["xs"], json!([10, 20, 30]));
}

#[tokio::test]
async fn pg_bigint_array_column_still_decodes() {
    // int8[] already worked before this fix; guards against regressing it
    // while widening the int2[]/int4[] handling above.
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request("GET", "/pg/arrays/bigint-column", None, &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["xs"], json!([100, 200, 300]));
}

/// Only `TEXT[]` was covered by the text-array branch; `VARCHAR[]` (and
/// `BPCHAR[]`/`CHAR[]`, `NAME[]`, `CITEXT[]`) fell through to the
/// integer-array attempt (which fails to decode a text array) and then to
/// the string fallbacks (which fail to decode any array as a scalar
/// `String`), silently coming back as JSON `null`.
#[tokio::test]
async fn pg_varchar_array_column_decodes() {
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request("GET", "/pg/arrays/varchar-column", None, &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["xs"], json!(["a", "b", "c"]));
}

#[tokio::test]
async fn pg_bpchar_array_column_decodes() {
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let (status, body) = app
        .request("GET", "/pg/arrays/bpchar-column", None, &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["xs"], json!(["a", "b"]));
}

#[tokio::test]
async fn pg_bulk_insert_via_arrays_single_round_trip() {
    // The 3c value proposition: caller pre-pivots data to column arrays,
    // one HTTP request → one INSERT ... SELECT unnest() → N rows.
    let url = require_pg!();
    let app = app_with_arrays(&url).await;
    let a = format!("bulk_a_{}", uuid());
    let b = format!("bulk_b_{}", uuid());
    let payload = json!({
        "actors": [a, b],
        "actions": ["created", "updated"],
    });
    let (status, body) = app
        .request(
            "POST",
            "/pg/arrays/bulk-insert",
            Some(&payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["action"], "created");
    assert_eq!(arr[1]["action"], "updated");
}

// ─── Issue #11: declared `items.type` drives array binding, so empty
// or all-null arrays don't silently fall back to JSONB (which can't
// insert into a native `text[]` / `int8[]` column). ─────────────────

async fn app_with_typed_arrays(url: &str) -> common::TestApp {
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_postgres_datasource("pg", url)
        .with_sql(
            "pg/POST/posts/create.sql",
            "/*\nparams:\n  title: { type: string, required: true }\n  \
             categories: { type: array, required: true, items: { type: string } }\n*/\n\
             INSERT INTO posts (title, categories) VALUES (:title, :categories) \
             RETURNING id, title, categories",
        )
        .with_sql(
            "pg/POST/posts/create-optional-cats.sql",
            "/*\nparams:\n  title: { type: string, required: true }\n  \
             categories: { type: array, required: false, items: { type: string } }\n*/\n\
             INSERT INTO posts (title, categories) \
             VALUES (:title, COALESCE(:categories, ARRAY[]::TEXT[])) \
             RETURNING id, title, categories",
        )
        .with_sql(
            "pg/POST/posts/create-typed-int.sql",
            // int8[] target column doesn't exist — but a `SELECT` of a
            // native int8[] round-trips the array shape without needing
            // one. Enough to prove the bind picks int8[] for an empty
            // declared-integer array.
            "/*\nparams:\n  xs: { type: array, required: true, items: { type: integer } }\n*/\n\
             SELECT :xs::BIGINT[] AS xs",
        )
        .build()
        .await
}

/// Reporter's exact scenario in #11: declared `items: { type: string }`,
/// caller sends `{"categories": []}`. Before this fix, the runtime
/// heuristic saw an empty array, gave up, and bound JSONB — Postgres
/// then refused to write JSONB into a `text[]` column (or silently
/// stored the JSON text encoding, per the reporter's observation).
/// With items.type in the mix, we bind `Vec::<Option<String>>::new()`
/// which sqlx encodes as an empty `text[]`.
#[tokio::test]
async fn pg_insert_empty_typed_array_into_text_array_column() {
    let url = require_pg!();
    let app = app_with_typed_arrays(&url).await;
    let payload = json!({
        "title": format!("emptycats_{}", uuid()),
        "categories": [],
    });
    let (status, body) = app
        .request("POST", "/pg/posts/create", Some(&payload.to_string()), &[])
        .await;
    assert_eq!(status, 200, "empty typed array insert must succeed: {body}");
    assert_eq!(body[0]["categories"], json!([]));
}

#[tokio::test]
async fn pg_insert_populated_typed_string_array_into_text_array_column() {
    let url = require_pg!();
    let app = app_with_typed_arrays(&url).await;
    let payload = json!({
        "title": format!("cats_{}", uuid()),
        "categories": ["tech", "rust", "postgres"],
    });
    let (status, body) = app
        .request("POST", "/pg/posts/create", Some(&payload.to_string()), &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["categories"], json!(["tech", "rust", "postgres"]));
}

/// Optional-null path: declared array param, `categories: null`. Bind
/// must produce a null `text[]` (not null JSONB) so the OID slot on
/// the cached prepared statement stays stable across null/non-null
/// calls — same OID-pinning invariant the scalar `#9` fix relies on,
/// extended to arrays.
#[tokio::test]
async fn pg_null_typed_array_then_populated_stays_cache_stable() {
    let url = require_pg!();
    let app = app_with_typed_arrays(&url).await;

    let payload_null = json!({
        "title": format!("nullcats_{}", uuid()),
        "categories": null,
    });
    let (status1, body1) = app
        .request(
            "POST",
            "/pg/posts/create-optional-cats",
            Some(&payload_null.to_string()),
            &[],
        )
        .await;
    assert_eq!(status1, 200, "first (null) call: {body1}");
    assert_eq!(body1[0]["categories"], json!([]));

    // Second call on the same cached statement with a populated array —
    // must not corrupt the connection or throw an OID-mismatch UTF-8
    // error.
    let payload_populated = json!({
        "title": format!("popcats_{}", uuid()),
        "categories": ["a", "b"],
    });
    let (status2, body2) = app
        .request(
            "POST",
            "/pg/posts/create-optional-cats",
            Some(&payload_populated.to_string()),
            &[],
        )
        .await;
    assert_eq!(
        status2, 200,
        "second (populated) call on same cached statement must not corrupt: {body2}"
    );
    assert_eq!(body2[0]["categories"], json!(["a", "b"]));
}

/// Element-type mismatch is rejected at the request boundary — before
/// any bind. The reporter's question in #11 (why should this even
/// pass?) is answered here: it doesn't.
#[tokio::test]
async fn pg_typed_array_wrong_element_type_returns_400() {
    let url = require_pg!();
    let app = app_with_typed_arrays(&url).await;
    let payload = json!({
        "title": format!("badcats_{}", uuid()),
        "categories": ["ok", 42, "also-ok"],
    });
    let (status, body) = app
        .request("POST", "/pg/posts/create", Some(&payload.to_string()), &[])
        .await;
    assert_eq!(status, 400, "wrong element type must 400: {body}");
    assert_eq!(body["error"], "InvalidParameterTypeException");
    assert!(
        body["message"].as_str().unwrap().contains("categories[1]"),
        "message must name the failing element index: {body}",
    );
}

#[tokio::test]
async fn pg_empty_typed_integer_array_binds_as_bigint_array() {
    let url = require_pg!();
    let app = app_with_typed_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/posts/create-typed-int",
            Some(r#"{"xs": []}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200, "empty typed-integer array must bind: {body}");
    assert_eq!(body[0]["xs"], json!([]));
}

// ─── Corners 3 & 4 (audit follow-up): real INSERTs into native
// int8[]/float8[]/bool[] columns, plus null→populated cache-stability
// coverage for the same element types. The text[] path was the only
// one exercised end-to-end before this — the OID mechanics for the
// other element types are identical (`Vec<Option<T>>` →
// `T::array_type_info()`) so a mismatch would show up here first. ──

async fn app_with_array_targets(url: &str) -> common::TestApp {
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_postgres_datasource("pg", url)
        .with_sql(
            "pg/POST/array-targets/insert-ints.sql",
            "/*\nparams:\n  ints: { type: array, required: false, items: { type: integer } }\n*/\n\
             INSERT INTO array_targets (ints) \
             VALUES (COALESCE(:ints, ARRAY[]::BIGINT[])) \
             RETURNING id, ints",
        )
        .with_sql(
            "pg/POST/array-targets/insert-floats.sql",
            "/*\nparams:\n  floats: { type: array, required: false, items: { type: number } }\n*/\n\
             INSERT INTO array_targets (floats) \
             VALUES (COALESCE(:floats, ARRAY[]::DOUBLE PRECISION[])) \
             RETURNING id, floats",
        )
        .with_sql(
            "pg/POST/array-targets/insert-flags.sql",
            "/*\nparams:\n  flags: { type: array, required: false, items: { type: boolean } }\n*/\n\
             INSERT INTO array_targets (flags) \
             VALUES (COALESCE(:flags, ARRAY[]::BOOLEAN[])) \
             RETURNING id, flags",
        )
        .build()
        .await
}

/// Null → populated on a native `int8[]` column. The null bind must
/// pin the slot to `int8[]` OID so the follow-up populated bind
/// (which produces `int8[]` naturally) reuses the cached prepared
/// statement without an OID-mismatch UTF-8 error.
#[tokio::test]
async fn pg_int_array_null_then_populated_stays_cache_stable() {
    let url = require_pg!();
    let app = app_with_array_targets(&url).await;

    let (s1, b1) = app
        .request(
            "POST",
            "/pg/array-targets/insert-ints",
            Some(r#"{"ints": null}"#),
            &[],
        )
        .await;
    assert_eq!(s1, 200, "null int[] insert failed: {b1}");
    assert_eq!(b1[0]["ints"], json!([]));

    let (s2, b2) = app
        .request(
            "POST",
            "/pg/array-targets/insert-ints",
            Some(r#"{"ints": [10, 20, 30]}"#),
            &[],
        )
        .await;
    assert_eq!(
        s2, 200,
        "populated int[] insert on same cached statement must not corrupt: {b2}"
    );
    assert_eq!(b2[0]["ints"], json!([10, 20, 30]));
}

#[tokio::test]
async fn pg_float_array_null_then_populated_stays_cache_stable() {
    let url = require_pg!();
    let app = app_with_array_targets(&url).await;

    let (s1, b1) = app
        .request(
            "POST",
            "/pg/array-targets/insert-floats",
            Some(r#"{"floats": null}"#),
            &[],
        )
        .await;
    assert_eq!(s1, 200, "null float[] insert failed: {b1}");
    assert_eq!(b1[0]["floats"], json!([]));

    let (s2, b2) = app
        .request(
            "POST",
            "/pg/array-targets/insert-floats",
            Some(r#"{"floats": [1.5, 2.5, 3.5]}"#),
            &[],
        )
        .await;
    assert_eq!(
        s2, 200,
        "populated float[] insert on same cached statement must not corrupt: {b2}"
    );
    assert_eq!(b2[0]["floats"], json!([1.5, 2.5, 3.5]));
}

#[tokio::test]
async fn pg_bool_array_null_then_populated_stays_cache_stable() {
    let url = require_pg!();
    let app = app_with_array_targets(&url).await;

    let (s1, b1) = app
        .request(
            "POST",
            "/pg/array-targets/insert-flags",
            Some(r#"{"flags": null}"#),
            &[],
        )
        .await;
    assert_eq!(s1, 200, "null bool[] insert failed: {b1}");
    assert_eq!(b1[0]["flags"], json!([]));

    let (s2, b2) = app
        .request(
            "POST",
            "/pg/array-targets/insert-flags",
            Some(r#"{"flags": [true, false, true]}"#),
            &[],
        )
        .await;
    assert_eq!(
        s2, 200,
        "populated bool[] insert on same cached statement must not corrupt: {b2}"
    );
    assert_eq!(b2[0]["flags"], json!([true, false, true]));
}

// ─── Scalar semantic-format validation (option "b" of the tightening
// pass): `type: uuid|date|datetime` scalars are now format-checked at
// the request boundary. Wire OID stays text so no SQL breaks, but a
// bad literal now returns `400 InvalidParameterTypeException` naming
// the failing param instead of the previous Postgres-side
// `BadSqlGrammarException`. ────────────────────────────────────────

async fn app_with_scalar_semantic_params(url: &str) -> common::TestApp {
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_postgres_datasource("pg", url)
        .with_sql(
            "pg/POST/scalars/uuid-echo.sql",
            "/*\nparams:\n  id: { type: uuid, required: true }\n*/\n\
             SELECT :id::UUID AS id",
        )
        .with_sql(
            "pg/POST/scalars/date-echo.sql",
            "/*\nparams:\n  d: { type: date, required: true }\n*/\n\
             SELECT :d::DATE AS d",
        )
        .with_sql(
            "pg/POST/scalars/datetime-echo.sql",
            "/*\nparams:\n  ts: { type: datetime, required: true }\n*/\n\
             SELECT :ts::TIMESTAMPTZ AS ts",
        )
        .build()
        .await
}

#[tokio::test]
async fn pg_scalar_uuid_valid_round_trips() {
    let url = require_pg!();
    let app = app_with_scalar_semantic_params(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/scalars/uuid-echo",
            Some(r#"{"id": "11111111-1111-1111-1111-111111111111"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["id"], "11111111-1111-1111-1111-111111111111");
}

/// The whole point of the tightening: a bad UUID literal is caught
/// at the request boundary as `InvalidParameterTypeException`, not
/// as a Postgres `BadSqlGrammarException` after the bind reaches
/// the server. Callers can distinguish "client sent garbage" from
/// "SQL is broken" by the exception name alone.
#[tokio::test]
async fn pg_scalar_uuid_bad_literal_returns_400_at_boundary() {
    let url = require_pg!();
    let app = app_with_scalar_semantic_params(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/scalars/uuid-echo",
            Some(r#"{"id": "not-a-uuid"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(
        body["error"], "InvalidParameterTypeException",
        "must surface as a boundary validation error, not a Postgres grammar error: {body}",
    );
    assert!(
        body["message"].as_str().unwrap().contains("'id'"),
        "message must name the failing param: {body}",
    );
}

#[tokio::test]
async fn pg_scalar_date_valid_round_trips() {
    let url = require_pg!();
    let app = app_with_scalar_semantic_params(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/scalars/date-echo",
            Some(r#"{"d": "2026-07-01"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["d"], "2026-07-01");
}

#[tokio::test]
async fn pg_scalar_date_bad_literal_returns_400_at_boundary() {
    let url = require_pg!();
    let app = app_with_scalar_semantic_params(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/scalars/date-echo",
            Some(r#"{"d": "2026-13-45"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "InvalidParameterTypeException");
}

#[tokio::test]
async fn pg_scalar_datetime_valid_round_trips() {
    let url = require_pg!();
    let app = app_with_scalar_semantic_params(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/scalars/datetime-echo",
            Some(r#"{"ts": "2026-01-01T10:20:30Z"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    let s = body[0]["ts"].as_str().unwrap();
    assert!(s.starts_with("2026-01-01T10:20:30"), "got {s}");
    assert!(s.ends_with('Z'));
}

#[tokio::test]
async fn pg_scalar_datetime_bad_literal_returns_400_at_boundary() {
    let url = require_pg!();
    let app = app_with_scalar_semantic_params(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/scalars/datetime-echo",
            Some(r#"{"ts": "sometime yesterday"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "InvalidParameterTypeException");
}

// ─── Corner 5: semantic-type arrays bind natively (uuid[], date[],
// timestamptz[]) so callers no longer need `::uuid[]` / `::date[]` /
// `::timestamptz[]` casts in the SQL. Read-side decodes match. Bad
// element formats surface at the request boundary with the failing
// element path. ────────────────────────────────────────────────────

async fn app_with_semantic_arrays(url: &str) -> common::TestApp {
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_postgres_datasource("pg", url)
        .with_sql(
            "pg/POST/semantic-arrays/insert-uuids.sql",
            "/*\nparams:\n  uuids: { type: array, required: false, items: { type: uuid } }\n*/\n\
             INSERT INTO semantic_arrays (uuids) \
             VALUES (COALESCE(:uuids, ARRAY[]::UUID[])) \
             RETURNING id, uuids",
        )
        .with_sql(
            "pg/POST/semantic-arrays/insert-dates.sql",
            "/*\nparams:\n  dates: { type: array, required: false, items: { type: date } }\n*/\n\
             INSERT INTO semantic_arrays (dates) \
             VALUES (COALESCE(:dates, ARRAY[]::DATE[])) \
             RETURNING id, dates",
        )
        .with_sql(
            "pg/POST/semantic-arrays/insert-timestamps.sql",
            "/*\nparams:\n  timestamps: { type: array, required: false, items: { type: datetime } }\n*/\n\
             INSERT INTO semantic_arrays (timestamps) \
             VALUES (COALESCE(:timestamps, ARRAY[]::TIMESTAMPTZ[])) \
             RETURNING id, timestamps",
        )
        .build()
        .await
}

/// Native uuid[] bind — no `::uuid[]` cast in the SQL, no implicit
/// text→uuid array coercion (which Postgres doesn't do). Round-trips
/// through the read path via the new `_UUID` decode branch.
#[tokio::test]
async fn pg_uuid_array_native_bind_and_decode() {
    let url = require_pg!();
    let app = app_with_semantic_arrays(&url).await;
    let payload = json!({
        "uuids": [
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222",
        ],
    });
    let (status, body) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-uuids",
            Some(&payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 200, "uuid[] insert failed: {body}");
    assert_eq!(
        body[0]["uuids"],
        json!([
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222"
        ])
    );
}

#[tokio::test]
async fn pg_uuid_array_null_then_populated_stays_cache_stable() {
    let url = require_pg!();
    let app = app_with_semantic_arrays(&url).await;

    let (s1, b1) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-uuids",
            Some(r#"{"uuids": null}"#),
            &[],
        )
        .await;
    assert_eq!(s1, 200, "null uuid[] insert: {b1}");
    assert_eq!(b1[0]["uuids"], json!([]));

    let (s2, b2) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-uuids",
            Some(r#"{"uuids": ["33333333-3333-3333-3333-333333333333"]}"#),
            &[],
        )
        .await;
    assert_eq!(
        s2, 200,
        "populated uuid[] on same cached statement must not corrupt: {b2}"
    );
    assert_eq!(
        b2[0]["uuids"],
        json!(["33333333-3333-3333-3333-333333333333"])
    );
}

#[tokio::test]
async fn pg_uuid_array_bad_element_returns_400() {
    let url = require_pg!();
    let app = app_with_semantic_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-uuids",
            Some(r#"{"uuids": ["11111111-1111-1111-1111-111111111111", "not-a-uuid"]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 400, "bad uuid element must 400: {body}");
    assert_eq!(body["error"], "InvalidParameterTypeException");
    assert!(
        body["message"].as_str().unwrap().contains("uuids[1]"),
        "message must name failing element: {body}",
    );
}

#[tokio::test]
async fn pg_date_array_native_bind_and_decode() {
    let url = require_pg!();
    let app = app_with_semantic_arrays(&url).await;
    let payload = json!({"dates": ["2026-07-01", "2026-07-02"]});
    let (status, body) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-dates",
            Some(&payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 200, "date[] insert failed: {body}");
    assert_eq!(body[0]["dates"], json!(["2026-07-01", "2026-07-02"]));
}

#[tokio::test]
async fn pg_date_array_null_then_populated_stays_cache_stable() {
    let url = require_pg!();
    let app = app_with_semantic_arrays(&url).await;

    let (s1, b1) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-dates",
            Some(r#"{"dates": null}"#),
            &[],
        )
        .await;
    assert_eq!(s1, 200, "null date[]: {b1}");
    assert_eq!(b1[0]["dates"], json!([]));

    let (s2, b2) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-dates",
            Some(r#"{"dates": ["2026-12-31"]}"#),
            &[],
        )
        .await;
    assert_eq!(s2, 200, "populated date[] must not corrupt: {b2}");
    assert_eq!(b2[0]["dates"], json!(["2026-12-31"]));
}

#[tokio::test]
async fn pg_date_array_bad_element_returns_400() {
    let url = require_pg!();
    let app = app_with_semantic_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-dates",
            Some(r#"{"dates": ["2026-07-01", "not-a-date"]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 400, "bad date element must 400: {body}");
    assert!(
        body["message"].as_str().unwrap().contains("dates[1]"),
        "message must name failing element: {body}",
    );
}

#[tokio::test]
async fn pg_timestamptz_array_native_bind_and_decode() {
    let url = require_pg!();
    let app = app_with_semantic_arrays(&url).await;
    let payload = json!({
        "timestamps": ["2026-01-01T10:20:30Z", "2026-02-15T05:00:00Z"],
    });
    let (status, body) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-timestamps",
            Some(&payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 200, "timestamptz[] insert failed: {body}");
    let arr = body[0]["timestamps"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // Round-trip preserves the wall-clock UTC instant. Fractional-
    // second suffix may or may not appear depending on how Postgres
    // stored it — assert on prefix + Z suffix rather than exact form.
    for (out, want_prefix) in arr
        .iter()
        .zip(["2026-01-01T10:20:30", "2026-02-15T05:00:00"])
    {
        let s = out.as_str().unwrap();
        assert!(
            s.starts_with(want_prefix),
            "got {s}, want prefix {want_prefix}"
        );
        assert!(s.ends_with('Z'), "must end with Z (UTC-anchored): {s}");
    }
}

#[tokio::test]
async fn pg_timestamptz_array_null_then_populated_stays_cache_stable() {
    let url = require_pg!();
    let app = app_with_semantic_arrays(&url).await;

    let (s1, b1) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-timestamps",
            Some(r#"{"timestamps": null}"#),
            &[],
        )
        .await;
    assert_eq!(s1, 200, "null timestamptz[]: {b1}");
    assert_eq!(b1[0]["timestamps"], json!([]));

    let (s2, b2) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-timestamps",
            Some(r#"{"timestamps": ["2026-06-15T12:00:00Z"]}"#),
            &[],
        )
        .await;
    assert_eq!(s2, 200, "populated timestamptz[] must not corrupt: {b2}");
    let s = b2[0]["timestamps"][0].as_str().unwrap();
    assert!(s.starts_with("2026-06-15T12:00:00"), "got {s}");
    assert!(s.ends_with('Z'), "got {s}");
}

#[tokio::test]
async fn pg_timestamptz_array_rfc3339_offset_normalises_to_utc() {
    let url = require_pg!();
    let app = app_with_semantic_arrays(&url).await;
    // +02:00 offset → UTC by subtracting 2 hours (08:20:30 UTC).
    let payload = json!({"timestamps": ["2026-01-01T10:20:30+02:00"]});
    let (status, body) = app
        .request(
            "POST",
            "/pg/semantic-arrays/insert-timestamps",
            Some(&payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    let s = body[0]["timestamps"][0].as_str().unwrap();
    assert!(
        s.starts_with("2026-01-01T08:20:30"),
        "offset must normalise to UTC (expected 08:20:30 UTC), got {s}"
    );
    assert!(s.ends_with('Z'), "must render with Z suffix, got {s}");
}

/// Empty typed-integer array insert into a real `int8[]` column
/// (weaker `SELECT :xs::BIGINT[]` variant of this test exists above;
/// this one closes the "prove it against a real column, not a cast"
/// gap flagged in the audit).
#[tokio::test]
async fn pg_empty_int_array_into_bigint_column() {
    let url = require_pg!();
    let app = app_with_array_targets(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/array-targets/insert-ints",
            Some(r#"{"ints": []}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200, "empty int[] insert failed: {body}");
    assert_eq!(body[0]["ints"], json!([]));
}

// ─── Issue #10: reporter's scenario. LIMIT/OFFSET declared as `number`,
// caller sends both as JSON strings, and the same cached statement is
// hit repeatedly with mixed shapes (default fallback, string coercion,
// numeric direct). All three shapes must bind identical OIDs (Float8)
// so the connection never surfaces `invalid byte sequence for encoding
// "UTF8"` on a later Bind. ───────────────────────────────────────────

async fn app_with_paginated_gates(url: &str) -> common::TestApp {
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_postgres_datasource("pg", url)
        .with_sql(
            "pg/POST/gates/list.sql",
            // Distilled from the reporter's SQL — same COALESCE(...::int)
            // shape around named number params with defaults. The
            // subselect body is stripped down to a literal so this test
            // doesn't need a `gates` table to exist.
            "/*\nparams:\n  limit: { type: number, default: 20 }\n  \
             offset: { type: number, default: 0 }\n*/\n\
             SELECT n FROM generate_series(1, 3) AS t(n) \
             LIMIT COALESCE(:limit::int, 20) OFFSET COALESCE(:offset::int, 0)",
        )
        .build()
        .await
}

/// The three input shapes callers actually use in the field —
/// default-fallback, string-coerced, and native number — issued
/// sequentially against the same cached prepared statement. Any one
/// producing a different OID than the others corrupts the slot for
/// every subsequent call on that connection.
#[tokio::test]
async fn pg_number_param_shape_mix_stays_cache_stable() {
    let url = require_pg!();
    let app = app_with_paginated_gates(&url).await;

    // 1. Body {}: both params fall through to declared defaults.
    let (s1, b1) = app.request("POST", "/pg/gates/list", Some("{}"), &[]).await;
    assert_eq!(s1, 200, "default-shape call: {b1}");
    assert_eq!(b1.as_array().unwrap().len(), 3);

    // 2. Strings — the reporter's usage. Coerces through `Number` →
    //    f64 → same Float8 OID.
    let (s2, b2) = app
        .request(
            "POST",
            "/pg/gates/list",
            Some(r#"{"limit": "2", "offset": "0"}"#),
            &[],
        )
        .await;
    assert_eq!(
        s2, 200,
        "string-shape call must not corrupt connection: {b2}"
    );
    assert_eq!(b2.as_array().unwrap().len(), 2);

    // 3. Native JSON numbers.
    let (s3, b3) = app
        .request(
            "POST",
            "/pg/gates/list",
            Some(r#"{"limit": 1, "offset": 1}"#),
            &[],
        )
        .await;
    assert_eq!(s3, 200, "numeric-shape call must succeed: {b3}");
    let rows = b3.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["n"], 2);

    // 4. Explicit null — floors to default via COALESCE, same OID.
    let (s4, b4) = app
        .request(
            "POST",
            "/pg/gates/list",
            Some(r#"{"limit": null, "offset": null}"#),
            &[],
        )
        .await;
    assert_eq!(s4, 200, "null-shape call must succeed: {b4}");
    assert_eq!(b4.as_array().unwrap().len(), 3);
}

/// Stress the shape mix over many iterations — the reporter's report
/// characterises the failure as "seemingly to no pattern", so if any
/// residual non-determinism remains we want to shake it out before
/// shipping. With the fix in place, every iteration binds Float8 for
/// each slot and Postgres never sees an OID mismatch.
#[tokio::test]
async fn pg_number_param_shape_mix_survives_repeated_alternation() {
    let url = require_pg!();
    let app = app_with_paginated_gates(&url).await;

    let shapes: &[Option<&str>] = &[
        Some("{}"),
        Some(r#"{"limit": "3", "offset": "0"}"#),
        Some(r#"{"limit": 1, "offset": 0}"#),
        Some(r#"{"limit": null, "offset": "1"}"#),
        Some(r#"{"limit": "2", "offset": null}"#),
    ];
    for iter in 0..40 {
        let payload = shapes[iter % shapes.len()];
        let (status, body) = app.request("POST", "/pg/gates/list", payload, &[]).await;
        assert_eq!(
            status, 200,
            "iteration {iter} with body {payload:?} failed: {body}"
        );
    }
}

// ─── Decode-completeness: `numeric[]` and `jsonb[]` columns. Same
// silent-null-decode failure mode as `float8[]`/`bool[]` had before
// their branches landed — every array flavour without an explicit
// decode branch fell through to the int8[] catch-all, failed, and
// the column came back as JSON null. ──────────────────────────────

async fn app_with_decode_arrays(url: &str) -> common::TestApp {
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_postgres_datasource("pg", url)
        .with_sql(
            "pg/POST/decode/numeric-array.sql",
            "INSERT INTO decimal_arrays (amounts) VALUES (:amounts::NUMERIC(12,2)[]) \
             RETURNING id, amounts",
        )
        .with_sql(
            "pg/POST/decode/jsonb-array.sql",
            "INSERT INTO jsonb_arrays (docs) VALUES (:docs::JSONB[]) \
             RETURNING id, docs",
        )
        .with_sql(
            "pg/GET/decode/numeric-array-literal.sql",
            "SELECT ARRAY[12345.67, 9876.54]::NUMERIC(12,2)[] AS amounts",
        )
        .with_sql(
            "pg/GET/decode/jsonb-array-literal.sql",
            "SELECT ARRAY['{\"k\":1}'::JSONB, '{\"k\":2}'::JSONB]::JSONB[] AS docs",
        )
        .build()
        .await
}

/// `numeric[]` decodes as JSON strings — same precision-preserving
/// treatment as scalar NUMERIC (f64 would silently truncate).
#[tokio::test]
async fn pg_numeric_array_column_decodes_as_strings() {
    let url = require_pg!();
    let app = app_with_decode_arrays(&url).await;
    let (status, body) = app
        .request("GET", "/pg/decode/numeric-array-literal", None, &[])
        .await;
    assert_eq!(status, 200);
    let arr = body[0]["amounts"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // Precision preserved — string form, not f64-rounded.
    assert_eq!(arr[0], "12345.67");
    assert_eq!(arr[1], "9876.54");
}

/// `jsonb[]` decodes each element as its native JSON value, not
/// stringified. Mirrors the scalar `JSONB` decode.
#[tokio::test]
async fn pg_jsonb_array_column_decodes_as_json_values() {
    let url = require_pg!();
    let app = app_with_decode_arrays(&url).await;
    let (status, body) = app
        .request("GET", "/pg/decode/jsonb-array-literal", None, &[])
        .await;
    assert_eq!(status, 200);
    let arr = body[0]["docs"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0], json!({"k": 1}));
    assert_eq!(arr[1], json!({"k": 2}));
}

/// End-to-end via a real `NUMERIC(12,2)[]` column — proves the
/// decode works against a column with a target precision, not just
/// an anonymous SELECT.
#[tokio::test]
async fn pg_numeric_array_round_trips_through_native_column() {
    let url = require_pg!();
    let app = app_with_decode_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/decode/numeric-array",
            Some(r#"{"amounts": "{100.25, 200.75}"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200, "numeric[] insert failed: {body}");
    assert_eq!(body[0]["amounts"], json!(["100.25", "200.75"]));
}

/// End-to-end via a real `JSONB[]` column.
#[tokio::test]
async fn pg_jsonb_array_round_trips_through_native_column() {
    let url = require_pg!();
    let app = app_with_decode_arrays(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/pg/decode/jsonb-array",
            Some(r#"{"docs": "{\"{\\\"a\\\": 1}\", \"{\\\"b\\\": 2}\"}"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200, "jsonb[] insert failed: {body}");
    assert_eq!(body[0]["docs"], json!([{"a": 1}, {"b": 2}]));
}

// ─── Task 003 on Postgres: @transactional single-shot rollback ───────

async fn app_with_tx_marker(url: &str) -> common::TestApp {
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_postgres_datasource("pg", url)
        .with_sql(
            "pg/POST/tx/insert-then-fail.sql",
            "-- @transactional\n\
             INSERT INTO audit_log (actor, action) VALUES (:actor, 'first'); \
             SELECT * FROM definitely_not_a_table",
        )
        .with_sql(
            "pg/POST/audit/append.sql",
            "INSERT INTO audit_log (actor, action) VALUES (:actor, :action) \
             RETURNING id",
        )
        .with_sql(
            "pg/GET/audit/count-by-actor.sql",
            "SELECT count(*) AS n FROM audit_log WHERE actor = :actor",
        )
        .build()
        .await
}

#[tokio::test]
async fn pg_transactional_marker_rolls_back_multistatement() {
    let url = require_pg!();
    let app = app_with_tx_marker(&url).await;
    let actor = format!("txmarker_{}", uuid());
    let (status, body) = app
        .request(
            "POST",
            "/pg/tx/insert-then-fail",
            Some(&json!({"actor": actor}).to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "BadSqlGrammarException");

    let (s2, cnt) = app
        .request(
            "GET",
            &format!("/pg/audit/count-by-actor?actor={actor}"),
            None,
            &[],
        )
        .await;
    assert_eq!(s2, 200);
    assert_eq!(
        cnt[0]["n"], 0,
        "@transactional must have rolled back the INSERT"
    );
}

fn uuid() -> String {
    // Cheap unique suffix — millis + a nanos tail — so parallel test bodies
    // don't collide on unique(login).
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    format!("{}{}", n.as_millis(), n.subsec_nanos())
}
