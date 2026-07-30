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
            "SELECT cast(:n AS INTEGER) AS coerced, :n::TEXT AS as_text",
        )
        .with_sql(
            "pg/POST/typechecks/null.sql",
            "SELECT :maybe IS NULL AS is_null, coalesce(:maybe, 'fallback') AS resolved",
        )
        .with_sql(
            "pg/POST/typechecks/bad-grammar.sql",
            "SELECT * FROM definitely_not_a_table",
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
async fn pg_timestamptz_serialises_to_string() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("GET", "/pg/users/created-at?login=alice", None, &[])
        .await;
    assert_eq!(status, 200);
    let created = body[0]["createdAt"].as_str().unwrap();
    // RFC3339 with timezone. Not asserting the exact instant — Liquibase
    // uses NOW() at seed time — only the shape.
    assert!(created.len() >= 19, "timestamp too short: {created}");
    assert!(created.contains('T') || created.contains(' '));
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

// ─── Errors ───────────────────────────────────────────────────────────

#[tokio::test]
async fn pg_missing_param_400() {
    let url = require_pg!();
    let app = app_with_pg(&url).await;
    let (status, body) = app
        .request("POST", "/pg/typechecks/null", Some("{}"), &[])
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "InvalidDataAccessApiUsageException");
    assert!(body["message"].as_str().unwrap().contains("'maybe'"));
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
    assert_eq!(entry["driver"], "postgres");
    // Password never leaks even when the URL has one.
    let url_field = entry["url"].as_str().unwrap();
    if url_field.contains('@') {
        assert!(
            url_field.contains("*****"),
            "password should be masked, saw: {url_field}"
        );
    }
}

fn uuid() -> String {
    // Cheap unique suffix — millis + a nanos tail — so parallel test bodies
    // don't collide on unique(login).
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    format!("{}{}", n.as_millis(), n.subsec_nanos())
}
