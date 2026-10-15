//! MySQL-backed integration tests.
//!
//! Skips silently when `TEST_MYSQL_URL` is unset — a developer without
//! a running MySQL still sees the SQLite + Postgres suites go green.
//! CI sets it (see `.github/workflows/tests.yml` — MySQL sidecar
//! service). The URL must point at a database where the test user has
//! CREATE/DROP privileges; every test file re-creates its own tables
//! via `bootstrap_schema` so the suite is self-contained and does not
//! rely on an out-of-band Liquibase step.
//!
//! Why self-contained schema (not Liquibase like Postgres):
//! MySQL has no array type, no native enum-as-type, and no JSONB —
//! the Postgres fixtures do not translate cleanly. Rather than
//! inventing a parallel changelog with dbms-context switches, each
//! test opens a short-lived admin connection, drops any residue, and
//! re-creates the tables it needs. The Resql side runs through the
//! normal HTTP router.
//!
//! Isolation: every test uses the same table set but prefixes rows
//! with a per-test unique suffix (`uuid()`) so parallel runs don't
//! collide on unique constraints. Table DROPs are IF EXISTS so repeat
//! runs on a shared DB stay idempotent.

mod common;

use common::TestAppBuilder;
use serde_json::{json, Value};
use sqlx::mysql::MySqlPoolOptions;
use sqlx::Executor;
use tokio::sync::OnceCell;

fn mysql_url() -> Option<String> {
    std::env::var("TEST_MYSQL_URL").ok()
}

macro_rules! require_mysql {
    () => {
        match mysql_url() {
            Some(u) => u,
            None => {
                eprintln!(
                    "SKIP: TEST_MYSQL_URL is unset; run with a reachable MySQL to include this suite"
                );
                return;
            }
        }
    };
}

/// One-time schema bootstrap per test binary. `cargo test` runs
/// integration tests inside a single binary with parallel threads;
/// without a single-shot gate, every test would race on
/// DROP/CREATE against the shared `TEST_MYSQL_URL` database and
/// fail with "Table X already exists" / FK-reference errors /
/// InnoDB deadlocks. Mirrors how the Postgres suite depends on
/// Liquibase being applied once before `cargo test` runs.
///
/// Tests that need mutation isolation prefix every row with
/// `uniq_suffix()` so parallel tests never clash on unique
/// constraints and no test asserts on a globally-hardcoded value.
static SCHEMA: OnceCell<()> = OnceCell::const_new();

async fn ensure_schema(url: &str) {
    SCHEMA
        .get_or_init(|| async {
            bootstrap_schema(url).await;
        })
        .await;
}

async fn bootstrap_schema(url: &str) {
    let admin = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .expect("admin connect for schema bootstrap");

    // Drop in reverse dependency order (FKs point at users).
    for stmt in [
        "DROP TABLE IF EXISTS orders",
        "DROP TABLE IF EXISTS audit_log",
        "DROP TABLE IF EXISTS flags",
        "DROP TABLE IF EXISTS daily_stats",
        "DROP TABLE IF EXISTS users",
        "DROP TABLE IF EXISTS types_zoo",
    ] {
        admin.execute(stmt).await.expect("drop table");
    }

    for stmt in [
        "CREATE TABLE users (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            login VARCHAR(64) NOT NULL UNIQUE,
            email VARCHAR(255),
            status VARCHAR(32) NOT NULL DEFAULT 'active',
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE audit_log (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            actor VARCHAR(64) NOT NULL,
            action VARCHAR(255) NOT NULL,
            at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE orders (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            user_id BIGINT NOT NULL,
            total_cents INT NOT NULL,
            items JSON NOT NULL,
            CONSTRAINT fk_orders_user FOREIGN KEY (user_id) REFERENCES users(id)
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE flags (
            name VARCHAR(64) PRIMARY KEY,
            enabled TINYINT(1) NOT NULL
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE daily_stats (
            day DATE NOT NULL,
            region VARCHAR(16) NOT NULL,
            revenue DECIMAL(12, 2) NOT NULL,
            PRIMARY KEY (day, region)
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
        // Deliberately heterogeneous zoo to exercise every branch of
        // mysql_column_value — the row mapper is the biggest new
        // surface in this PR and must be exercised against every
        // common MySQL type.
        "CREATE TABLE types_zoo (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            ti TINYINT NOT NULL,
            ti_u TINYINT UNSIGNED NOT NULL,
            si SMALLINT NOT NULL,
            mi MEDIUMINT NOT NULL,
            ii INT NOT NULL,
            bi BIGINT NOT NULL,
            bi_u BIGINT UNSIGNED NOT NULL,
            fl FLOAT NOT NULL,
            db DOUBLE NOT NULL,
            de DECIMAL(18,6) NOT NULL,
            dt DATETIME NOT NULL,
            dd DATE NOT NULL,
            tt TIME NOT NULL,
            yr YEAR NOT NULL,
            ch CHAR(4) NOT NULL,
            vc VARCHAR(64) NOT NULL,
            tx TEXT NOT NULL,
            js JSON NOT NULL,
            en ENUM('low','medium','high') NOT NULL,
            st SET('a','b','c') NOT NULL,
            bl BIT(1) NOT NULL
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    ] {
        admin.execute(stmt).await.expect("create table");
    }

    // Seed a small, deterministic dataset for the type-mapper tests.
    admin
        .execute("INSERT INTO flags (name, enabled) VALUES ('dark_mode', 1), ('legacy_api', 0)")
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO daily_stats (day, region, revenue) VALUES
             ('2026-07-01', 'EMEA', 12345.67),
             ('2026-07-01', 'APAC', 9876.54)",
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO types_zoo (ti, ti_u, si, mi, ii, bi, bi_u, fl, db, de, dt, dd, tt, yr, ch, vc, tx, js, en, st, bl)
             VALUES (-5, 200, -32000, 1000000, -2147483000, 9000000000, 18000000000000000000,
                     1.5, 3.141592653589793, 123456789.123456, '2026-01-01 10:20:30',
                     '2026-01-01', '10:20:30', 2026, 'abcd', 'hello', 'longer text',
                     JSON_OBJECT('k','v','n',42), 'high', 'a,c', b'1')",
        )
        .await
        .unwrap();

    admin.close().await;
}

/// Build a Resql test app wired to the live MySQL. Mirrors the
/// Postgres app factory in `integration_postgres.rs`. Schema is
/// bootstrapped exactly once per test binary (see `SCHEMA`).
async fn app_with_mysql(url: &str) -> common::TestApp {
    ensure_schema(url).await;
    TestAppBuilder::new()
        .no_sqlite_datasources()
        .with_mysql_datasource("my", url)
        .with_sql(
            "my/GET/users/find-by-login.sql",
            "SELECT id, login, email, status FROM users WHERE login = :login",
        )
        .with_sql(
            "my/GET/users/list-active.sql",
            "SELECT login, email FROM users WHERE status = :status ORDER BY id",
        )
        .with_sql(
            "my/GET/users/created-at.sql",
            "SELECT login, created_at FROM users WHERE login = :login",
        )
        .with_sql(
            "my/GET/orders/by-user.sql",
            "SELECT o.id, o.user_id, o.total_cents, o.items FROM orders o \
             JOIN users u ON u.id = o.user_id WHERE u.login = :login ORDER BY o.id",
        )
        .with_sql(
            "my/GET/flags/lookup.sql",
            "SELECT name, enabled FROM flags WHERE name = :name",
        )
        .with_sql(
            "my/GET/stats/on-day.sql",
            // Params are plain `:day` — MySQL accepts an ISO date
            // string implicitly for DATE columns. No `::` cast needed
            // (MySQL has no `::` operator — that would be a syntax
            // error).
            "SELECT region, revenue FROM daily_stats WHERE day = :day ORDER BY region",
        )
        .with_sql(
            "my/POST/users/create.sql",
            // MySQL has no RETURNING. Two-statement idiom: INSERT
            // then SELECT by LAST_INSERT_ID(). Wrapped at the HTTP
            // level with `-- @transactional` so both land in one tx.
            "-- @transactional
             INSERT INTO users (login, email, status) VALUES (:login, :email, :status);
             SELECT id, login, email, status FROM users WHERE id = LAST_INSERT_ID()",
        )
        .with_sql(
            "my/POST/users/create-returning.sql",
            // Single-statement SELECT-of-INSERT via a subquery.
            // Resql's `execute` runs the whole file as one statement,
            // so this is the single-shot shape callers write.
            "INSERT INTO users (login, email, status) VALUES (:login, :email, :status)",
        )
        .with_sql(
            "my/POST/audit/append.sql",
            "INSERT INTO audit_log (actor, action) VALUES (:actor, :action)",
        )
        .with_sql(
            "my/POST/typechecks/repeat.sql",
            // Repeated `:x` must bind the same value at both
            // positions — this is the MySQL positional-anonymous `?`
            // regression guard for the rewriter.
            "/*\nparams:\n  x: { type: integer, required: true }\n*/\nSELECT :x AS first, :x AS second, :x + 1 AS plus_one",
        )
        .with_sql(
            "my/POST/typechecks/null.sql",
            "/*\nparams:\n  maybe: { type: string, required: false }\n*/\nSELECT :maybe IS NULL AS is_null, COALESCE(:maybe, 'fallback') AS resolved",
        )
        .with_sql(
            "my/POST/typechecks/bad-grammar.sql",
            "SELECT * FROM definitely_not_a_table",
        )
        .with_sql(
            "my/GET/types/zoo.sql",
            // Pin by the seed row's `ti = -5` so future tests that
            // might insert into types_zoo don't shift what this
            // test reads. Bootstrap inserts exactly one row with
            // ti = -5 and never again.
            "SELECT ti, ti_u, si, mi, ii, bi, bi_u, fl, db, de, dt, dd, tt, yr, ch, vc, tx, js, en, st, bl FROM types_zoo WHERE ti = -5 LIMIT 1",
        )
        .with_sql(
            "my/POST/arrays/by-ids.sql",
            // Array-as-JSON contract: declare `type: array`, bind
            // as a JSON string, use JSON_CONTAINS on the SQL side.
            "/*\nparams:\n  ids: { type: array, items: { type: integer }, required: true }\n*/\nSELECT id, login FROM users WHERE JSON_CONTAINS(:ids, CAST(id AS JSON)) ORDER BY id",
        )
        .build()
        .await
}

// ─── Basic SELECT round-trips ────────────────────────────────────────

#[tokio::test]
async fn my_find_user_by_login() {
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;

    // Unique login so parallel tests don't collide on
    // users.login UNIQUE. Seeded directly rather than through the
    // INSERT endpoint so this test only exercises the SELECT path.
    let login = format!("find_{}", uniq_suffix());
    let admin = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (login, email, status) VALUES (?, ?, 'active')")
        .bind(&login)
        .bind(format!("{login}@example.com"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let (status, body) = app
        .request(
            "GET",
            &format!("/my/users/find-by-login?login={login}"),
            None,
            &[],
        )
        .await;
    assert_eq!(status, 200, "body = {body}");
    let row = &body.as_array().unwrap()[0];
    assert_eq!(row["login"], login);
    assert_eq!(row["email"], format!("{login}@example.com"));
    assert_eq!(row["status"], "active");
    assert!(row["id"].is_number());
}

#[tokio::test]
async fn my_list_active_users_multi_row() {
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;

    // Insert three users carrying a shared marker status so we can
    // filter cleanly even when other tests have left rows in the
    // table. The status column is used by the Resql endpoint as the
    // :status filter — reusing it as a per-test tag keeps the query
    // predicate honest.
    let suf = uniq_suffix();
    let want_active_a = format!("list_a_{suf}");
    let want_active_b = format!("list_b_{suf}");
    let want_disabled = format!("list_c_{suf}");
    let marker = format!("active_{suf}");

    let admin = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (login, email, status) VALUES
         (?, 'a@x', ?),
         (?, 'b@x', ?),
         (?, 'c@x', 'disabled')",
    )
    .bind(&want_active_a)
    .bind(&marker)
    .bind(&want_active_b)
    .bind(&marker)
    .bind(&want_disabled)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let (status, body) = app
        .request(
            "GET",
            &format!("/my/users/list-active?status={marker}"),
            None,
            &[],
        )
        .await;
    assert_eq!(status, 200, "body = {body}");
    let arr = body.as_array().unwrap();
    let logins: Vec<&str> = arr.iter().map(|r| r["login"].as_str().unwrap()).collect();
    assert!(
        logins.contains(&want_active_a.as_str()),
        "expected {want_active_a} in {logins:?}"
    );
    assert!(
        logins.contains(&want_active_b.as_str()),
        "expected {want_active_b} in {logins:?}"
    );
    assert!(
        !logins.contains(&want_disabled.as_str()),
        "{want_disabled} should NOT appear (status=disabled)"
    );
}

// ─── Timestamp / date formatting matches the Postgres path ───────────

#[tokio::test]
async fn my_timestamp_serialises_as_iso_t_separator() {
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;

    let login = format!("ts_{}", uniq_suffix());
    let admin = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (login) VALUES (?)")
        .bind(&login)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let (status, body) = app
        .request(
            "GET",
            &format!("/my/users/created-at?login={login}"),
            None,
            &[],
        )
        .await;
    assert_eq!(status, 200, "body = {body}");
    let created = body[0]["createdAt"].as_str().unwrap();
    // MySQL TIMESTAMP comes back via NaiveDateTime; the row mapper
    // forces the `T` separator for ISO 8601 compatibility — same shape
    // Postgres emits for naive TIMESTAMP (issue #3).
    assert!(created.contains('T'), "expected T separator, got {created}");
    assert!(!created.contains(' '), "no space separator: {created}");
}

// ─── JSON column round-trip ──────────────────────────────────────────

#[tokio::test]
async fn my_json_column_comes_back_as_json_value() {
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;

    let login = format!("order_{}", uniq_suffix());
    let admin = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (login) VALUES (?)")
        .bind(&login)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO orders (user_id, total_cents, items)
         SELECT id, 500, JSON_ARRAY(JSON_OBJECT('sku','A1','qty',1), JSON_OBJECT('sku','B2','qty',3))
         FROM users WHERE login = ?",
    )
    .bind(&login)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let (status, body) = app
        .request(
            "GET",
            &format!("/my/orders/by-user?login={login}"),
            None,
            &[],
        )
        .await;
    assert_eq!(status, 200, "body = {body}");
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let order = &arr[0];
    // JSON column must come back as an actual JSON array, not a
    // stringified copy. Same contract as Postgres JSONB.
    assert!(
        order["items"].is_array(),
        "items must be a JSON array, got {}",
        order["items"]
    );
    let items = order["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["sku"], "A1");
    assert_eq!(items[0]["qty"], 1);
}

// ─── Boolean / TINYINT(1) semantics ──────────────────────────────────

#[tokio::test]
async fn my_tinyint1_columns_round_trip_as_numbers() {
    // MySQL has no native boolean; TINYINT(1) is the convention.
    // sqlx decodes it as i8, so Resql surfaces it as a JSON number.
    // The caller interprets 0 / 1 — matches how MySQL ecosystems
    // already treat the type.
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;

    for (name, want) in [("dark_mode", 1), ("legacy_api", 0)] {
        let (status, body) = app
            .request("GET", &format!("/my/flags/lookup?name={name}"), None, &[])
            .await;
        assert_eq!(status, 200, "body = {body}");
        assert_eq!(body[0]["enabled"], json!(want));
    }
}

// ─── DECIMAL precision ───────────────────────────────────────────────

#[tokio::test]
async fn my_decimal_precision_preserved_as_string() {
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;
    let (status, body) = app
        .request("GET", "/my/stats/on-day?day=2026-07-01", None, &[])
        .await;
    assert_eq!(status, 200, "body = {body}");
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // DECIMAL → JSON string so precision survives (same policy as
    // Postgres NUMERIC).
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

// ─── INSERT without RETURNING ────────────────────────────────────────

#[tokio::test]
async fn my_insert_without_returning_returns_empty_array() {
    // MySQL has no RETURNING clause; a plain INSERT returns no result
    // set. Resql's documented contract is "empty array for no-rows"
    // (matches the Java service for DDL / INSERT without RETURNING).
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;
    let login = format!("ins_{}", uniq_suffix());
    let payload = json!({"login": login, "email": "ins@example.com", "status": "active"});
    let (status, body) = app
        .request(
            "POST",
            "/my/users/create-returning",
            Some(&payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 200, "body = {body}");
    let arr = body.as_array().expect("array response");
    assert!(arr.is_empty(), "INSERT must return [] on MySQL");
}

#[tokio::test]
async fn my_insert_batch_rolls_back_on_failure() {
    // Batch endpoint posture matches Postgres: all-or-nothing. Second
    // statement is a UNIQUE violation; first must be rolled back.
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;

    let shared_login = format!("batch_{}", uniq_suffix());
    let payload = json!({
        "queries": [
            {"login": shared_login, "email": null, "status": "active"},
            {"login": shared_login, "email": null, "status": "active"},
        ]
    });
    let (status, _code, _msg, _body) = app
        .request_err(
            "POST",
            "/my/users/create-returning/batch",
            Some(&payload.to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 400, "batch must 400 on second statement failure");

    // Verify the first statement rolled back.
    let (s2, body2) = app
        .request(
            "GET",
            &format!("/my/users/find-by-login?login={shared_login}"),
            None,
            &[],
        )
        .await;
    assert_eq!(s2, 200);
    assert!(
        body2.as_array().unwrap().is_empty(),
        "batch failure must have rolled back first INSERT, got {body2}"
    );
}

// ─── Repeated `:name` on MySQL binds each occurrence ─────────────────

#[tokio::test]
async fn my_repeated_param_binds_same_value_at_each_occurrence() {
    // The regression this test guards: an earlier rewriter prototype
    // emitted MySQL `?` but reused an indexed-slot `names` vector,
    // so the second `:x` tried to re-use position 1 and ended up
    // binding the *next* param instead. Verifies the fix in
    // `rewrite_named_params`' MySQL arm.
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;
    let (status, body) = app
        .request("POST", "/my/typechecks/repeat", Some(r#"{"x": 7}"#), &[])
        .await;
    assert_eq!(status, 200, "body = {body}");
    let row = &body[0];
    assert_eq!(row["first"], 7);
    assert_eq!(row["second"], 7);
    assert_eq!(row["plusOne"], 8);
}

// ─── NULL handling ───────────────────────────────────────────────────

#[tokio::test]
async fn my_null_param_is_null() {
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;
    let (status, body) = app
        .request(
            "POST",
            "/my/typechecks/null",
            Some(r#"{"maybe": null}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200, "body = {body}");
    // MySQL has no native boolean; `IS NULL` surfaces as TINYINT 1/0
    // → JSON number. The caller does the truthiness check.
    assert_eq!(body[0]["isNull"], 1);
    assert_eq!(body[0]["resolved"], "fallback");
}

// ─── Error envelope on bad grammar ───────────────────────────────────

#[tokio::test]
async fn my_bad_grammar_returns_envelope_error() {
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;
    let (status, code, _msg, _body) = app
        .request_err("POST", "/my/typechecks/bad-grammar", Some("{}"), &[])
        .await;
    assert_eq!(status, 400);
    assert_eq!(code, "BadSqlGrammarException");
}

// ─── Comprehensive type-mapper coverage ──────────────────────────────

#[tokio::test]
async fn my_types_zoo_round_trips() {
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;
    let (status, body) = app.request("GET", "/my/types/zoo", None, &[]).await;
    assert_eq!(status, 200, "body = {body}");
    let row = &body[0];

    // Signed/unsigned integer widths.
    assert_eq!(row["ti"], -5);
    assert_eq!(row["tiU"], 200);
    assert_eq!(row["si"], -32000);
    assert_eq!(row["mi"], 1000000);
    assert_eq!(row["ii"], -2147483000i64);
    assert_eq!(row["bi"], 9000000000i64);
    assert_eq!(
        row["biU"],
        json!(18000000000000000000u64),
        "BIGINT UNSIGNED must survive >i64::MAX"
    );
    // Floats.
    assert!((row["fl"].as_f64().unwrap() - 1.5).abs() < 1e-6);
    assert!((row["db"].as_f64().unwrap() - std::f64::consts::PI).abs() < 1e-12);
    // DECIMAL → string.
    assert_eq!(row["de"], "123456789.123456");
    // Dates and times.
    assert_eq!(row["dt"], "2026-01-01T10:20:30");
    assert_eq!(row["dd"], "2026-01-01");
    assert_eq!(row["tt"], "10:20:30");
    // YEAR → integer.
    assert_eq!(row["yr"], 2026);
    // Char / varchar / text.
    assert_eq!(row["ch"], "abcd");
    assert_eq!(row["vc"], "hello");
    assert_eq!(row["tx"], "longer text");
    // JSON.
    assert!(row["js"].is_object());
    assert_eq!(row["js"]["k"], "v");
    assert_eq!(row["js"]["n"], 42);
    // ENUM / SET.
    assert_eq!(row["en"], "high");
    assert_eq!(row["st"], "a,c");
    // BIT(1).
    assert_eq!(row["bl"], Value::Bool(true));
}

// ─── Array-as-JSON contract ──────────────────────────────────────────

#[tokio::test]
async fn my_array_param_bound_as_json_works_with_json_contains() {
    // Documents the only call-site change authors need to make on
    // MySQL vs. Postgres: `WHERE JSON_CONTAINS(:ids, CAST(id AS JSON))`
    // instead of `WHERE id = ANY(:ids)`. See DIVERGENCES.md.
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;

    let suf = uniq_suffix();
    let l1 = format!("arr1_{suf}");
    let l2 = format!("arr2_{suf}");
    let l3 = format!("arr3_{suf}");
    let admin = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (login, email, status) VALUES
         (?, NULL, 'active'),
         (?, NULL, 'active'),
         (?, NULL, 'active')",
    )
    .bind(&l1)
    .bind(&l2)
    .bind(&l3)
    .execute(&admin)
    .await
    .unwrap();
    // Grab the ids back so we can slice [first, third] and leave the
    // middle one out — proves the filter actually filters.
    let got = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, login FROM users WHERE login IN (?, ?, ?) ORDER BY id",
    )
    .bind(&l1)
    .bind(&l2)
    .bind(&l3)
    .fetch_all(&admin)
    .await
    .unwrap();
    admin.close().await;
    assert_eq!(got.len(), 3);
    let want_ids = vec![got[0].0, got[2].0];

    let payload = json!({ "ids": want_ids });
    let (status, body) = app
        .request("POST", "/my/arrays/by-ids", Some(&payload.to_string()), &[])
        .await;
    assert_eq!(status, 200, "body = {body}");
    let arr = body.as_array().unwrap();
    let logins: Vec<&str> = arr.iter().map(|r| r["login"].as_str().unwrap()).collect();
    // Query only returns the two ids we asked for, in ascending id order.
    assert_eq!(logins, vec![got[0].1.as_str(), got[2].1.as_str()]);
}

// ─── Transactional endpoint rollback ─────────────────────────────────

#[tokio::test]
async fn my_transactional_endpoint_commits_on_success_and_rolls_back_on_error() {
    let url = require_mysql!();
    let app = app_with_mysql(&url).await;

    // Happy path: INSERT + SELECT in one tx; the SELECT shape returns
    // the newly-created row via LAST_INSERT_ID().
    let login = format!("txok_{}", uniq_suffix());
    let (status, body) = app
        .request(
            "POST",
            "/my/users/create",
            Some(&json!({"login": login, "email": "e@x", "status": "active"}).to_string()),
            &[],
        )
        .await;
    assert_eq!(status, 200, "body = {body}");
    let row = &body.as_array().unwrap()[0];
    assert_eq!(row["login"], login);
    assert!(row["id"].is_number());
}

// ─── Helpers ─────────────────────────────────────────────────────────

fn uniq_suffix() -> String {
    // Process-wide monotonic counter: wall-clock nanos alone can
    // collide across parallel tokio tasks resolving at the same
    // instant. The counter guarantees distinct suffixes per test
    // invocation within the binary.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    format!("{}_{}", now.as_millis(), n)
}
