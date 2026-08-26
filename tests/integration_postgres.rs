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
            "/*\nparams:\n  maybe: { type: integer, required: false }\n*/\nSELECT :maybe IS NULL AS is_null, :maybe::INTEGER AS resolved",
        )
        .with_sql(
            "pg/POST/orders/create.sql",
            "/*\nparams:\n  login: { type: string, required: true }\n  total_cents: { type: integer, required: true }\n  items: { type: array, required: true }\n*/\n\
             INSERT INTO orders (user_id, total_cents, items) \
             VALUES ((SELECT id FROM users WHERE login = :login), :total_cents::INTEGER, :items) \
             RETURNING id, user_id, total_cents, items",
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

/// Documents the correct DSL-side fix for inserting a JSON number into a
/// plain `INTEGER` column: an explicit `::INTEGER` cast, exactly like
/// every other numeric-column example in this codebase. Since `bind_pg`
/// binds JSON numbers as text (see its doc comment), a bare
/// `INSERT ... VALUES (:n, ...)` against an `INTEGER` column with no
/// cast fails with `column "n" is of type integer but expression is of
/// type text` -- that failure is expected and correct; this test asserts
/// the supported shape (with the cast) succeeds.
#[tokio::test]
async fn pg_insert_number_into_integer_column_with_explicit_cast() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let payload = json!({
        "login": format!("orderuser_{}", uuid()),
        "email": "orders@example.com",
        "status": "active",
    });
    let (status, body) = app
        .request("POST", "/pg/users/create", Some(&payload.to_string()), &[])
        .await;
    assert_eq!(status, 200);
    let login = body[0]["login"].as_str().unwrap().to_string();

    let order_payload = json!({
        "login": login,
        "total_cents": 1500,
        "items": [{"sku": "X1", "qty": 1}],
    });
    let (status, body) = app
        .request(
            "POST",
            "/pg/orders/create",
            Some(&order_payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(
        status, 200,
        "INSERT of a JSON number into an INTEGER column, with an explicit \
         ::INTEGER cast on the SQL side, must succeed; got {status}: {body}"
    );
    let row = &body.as_array().unwrap()[0];
    assert_eq!(row["totalCents"], 1500);
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

/// Guards against a connection-corrupting regression in `bind_pg`: sqlx
/// caches prepared statements per SQL text on a connection, and Postgres
/// fixes each `$N` placeholder's type on the *first* `Parse` of that
/// cached statement. An optional `integer`/`number` parameter that is
/// `null` on one call and an actual number on a later call — both
/// against the *same* cached statement on the *same* connection — must
/// not corrupt the connection. Before the fix, the first call bound
/// `NULL` as an untyped/text placeholder (pinning that slot's type), and
/// the second call's native `i64` bind sent raw binary bytes into a slot
/// Postgres still expected as text, surfacing as `invalid byte sequence
/// for encoding "UTF8": 0x00`.
///
/// The test pool is pinned to a single connection (see
/// `TestAppBuilder::with_postgres_datasource`/`tests/common/mod.rs`) so
/// both calls deterministically land on the same physical connection —
/// with a multi-connection pool this reproduces only probabilistically,
/// depending on which connection the pool hands back for the second call.
#[tokio::test]
async fn pg_number_after_null_on_same_cached_statement_does_not_corrupt() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;

    let (status1, body1) = app
        .request("POST", "/pg/typechecks/maybe-number", Some("{}"), &[])
        .await;
    assert_eq!(status1, 200, "first (null) call failed: {body1}");
    assert_eq!(body1[0]["isNull"], true);

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
