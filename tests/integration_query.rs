mod common;
use common::TestAppBuilder;
use serde_json::json;

#[tokio::test]
async fn get_query_with_query_string_params() {
    let app = TestAppBuilder::new()
        .with_seed(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, login TEXT, email TEXT); \
             INSERT INTO users VALUES (1, 'admin', 'admin@example.com');",
        )
        .with_sql(
            "demo/GET/users/find-by-login.sql",
            "SELECT id, login, email FROM users WHERE login = :login",
        )
        .build()
        .await;
    let (status, body) = app
        .request("GET", "/demo/users/find-by-login?login=admin", None, &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!([{"id": 1, "login": "admin", "email": "admin@example.com"}])
    );
}

#[tokio::test]
async fn post_query_with_json_body() {
    let app = TestAppBuilder::new()
        .with_seed(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, login TEXT, email TEXT); \
             INSERT INTO users VALUES (1, 'admin', 'admin@example.com');",
        )
        .with_sql(
            "demo/POST/users/find-by-login.sql",
            "SELECT email FROM users WHERE login = :login",
        )
        .build()
        .await;
    let (status, body) = app
        .request(
            "POST",
            "/demo/users/find-by-login",
            Some(r#"{"login":"admin"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"email": "admin@example.com"}]));
}

#[tokio::test]
async fn missing_param_returns_400_named_shape() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT :login AS l")
        .build()
        .await;
    let (status, body) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "InvalidDataAccessApiUsageException");
    assert!(body["message"].as_str().unwrap().contains("'login'"));
}

#[tokio::test]
async fn query_not_found_returns_400() {
    let app = TestAppBuilder::new().build().await;
    let (status, body) = app.request("POST", "/nope/nope", Some("{}"), &[]).await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "ResqlRuntimeException");
    assert!(body["message"].as_str().unwrap().contains("'/nope/nope'"));
}

#[tokio::test]
async fn unknown_datasource_returns_400() {
    let app = TestAppBuilder::new()
        .with_datasources(&["demo"])
        .with_sql("otherproject/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, body) = app
        .request("POST", "/otherproject/x", Some("{}"), &[])
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "UnknownDataSourceNameException");
    assert!(body["message"].as_str().unwrap().contains("'otherproject'"));
}

#[tokio::test]
async fn sql_error_returns_400() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT * FROM does_not_exist")
        .build()
        .await;
    let (status, body) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "BadSqlGrammarException");
}

#[tokio::test]
async fn ddl_returns_empty_array() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/create.sql", "CREATE TABLE tmp_ddl (id INTEGER)")
        .build()
        .await;
    let (status, body) = app.request("POST", "/demo/create", Some("{}"), &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn snake_case_columns_come_back_camelcased() {
    let app = TestAppBuilder::new()
        .with_seed(
            "CREATE TABLE u (user_id INTEGER, password_hash TEXT); \
             INSERT INTO u VALUES (7, 'abc');",
        )
        .with_sql("demo/GET/u.sql", "SELECT user_id, password_hash FROM u")
        .build()
        .await;
    let (status, body) = app.request("GET", "/demo/u", None, &[]).await;
    assert_eq!(status, 200);
    let obj = &body.as_array().unwrap()[0];
    assert!(obj.get("userId").is_some());
    assert!(obj.get("passwordHash").is_some());
    assert!(obj.get("user_id").is_none());
}

#[tokio::test]
async fn null_json_body_is_treated_as_empty_object() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, body) = app.request("POST", "/demo/x", None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"n": 1}]));
}

#[tokio::test]
async fn malformed_json_returns_400() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, _body) = app.request("POST", "/demo/x", Some("{not json"), &[]).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn extra_body_params_ignored() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT :used AS u")
        .build()
        .await;
    let (status, body) = app
        .request(
            "POST",
            "/demo/x",
            Some(r#"{"used": 42, "extra": "hi"}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"u": 42}]));
}

#[tokio::test]
async fn batch_returns_array_of_arrays() {
    let app = TestAppBuilder::new()
        .with_seed(
            "CREATE TABLE t (login TEXT, email TEXT); \
             INSERT INTO t VALUES ('a', 'a@x'); \
             INSERT INTO t VALUES ('b', 'b@x'); \
             INSERT INTO t VALUES ('c', 'c@x');",
        )
        .with_sql(
            "demo/POST/lookup.sql",
            "SELECT email FROM t WHERE login = :login",
        )
        .build()
        .await;
    let (status, body) = app
        .request(
            "POST",
            "/demo/lookup/batch",
            Some(r#"{"queries": [{"login":"a"}, {"login":"b"}, {"login":"c"}]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!([
            [{"email": "a@x"}],
            [{"email": "b@x"}],
            [{"email": "c@x"}],
        ])
    );
}

#[tokio::test]
async fn batch_with_missing_param_returns_400() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/lookup.sql", "SELECT :login AS l")
        .build()
        .await;
    let (status, body) = app
        .request(
            "POST",
            "/demo/lookup/batch",
            Some(r#"{"queries":[{"login":"a"},{}]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "InvalidDataAccessApiUsageException");
}

#[tokio::test]
async fn batch_rolls_back_on_error_no_partial_writes() {
    // Task 007-3a: batch of 3 inserts, 2nd violates UNIQUE constraint.
    // After the 400, the table must have zero rows — no partial commits.
    let app = TestAppBuilder::new()
        .with_seed("CREATE TABLE t (login TEXT UNIQUE);")
        .with_sql(
            "demo/POST/add.sql",
            "INSERT INTO t (login) VALUES (:login) RETURNING login",
        )
        .with_sql("demo/GET/count.sql", "SELECT count(*) AS n FROM t")
        .build()
        .await;

    // 'a' then 'a' again then 'c' — second insert fails.
    let (status, body) = app
        .request(
            "POST",
            "/demo/add/batch",
            Some(r#"{"queries":[{"login":"a"},{"login":"a"},{"login":"c"}]}"#),
            &[],
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "BadSqlGrammarException");

    // The first insert must have rolled back with the failing one — 0 rows.
    let (s2, count_body) = app.request("GET", "/demo/count", None, &[]).await;
    assert_eq!(s2, 200);
    assert_eq!(count_body[0]["n"], 0);
}

#[tokio::test]
async fn transactional_marker_rolls_back_on_error() {
    // Task 003: an `-- @transactional` POST that fails leaves the DB clean.
    // Uses a UNIQUE constraint + one INSERT + one deliberately-bad INSERT
    // in a multi-statement body (SQLite executes them as separate rows via
    // repeated iteration is not possible; instead we prove the tx wrapper
    // fires by constructing an INSERT that succeeds followed by a SELECT
    // that fails — the failing SELECT rolls back the earlier INSERT).
    let app = TestAppBuilder::new()
        .with_seed("CREATE TABLE t (id INTEGER);")
        .with_sql(
            "demo/POST/tx-fail.sql",
            "-- @transactional\n\
             INSERT INTO t (id) VALUES (1); \
             SELECT * FROM nonexistent_table",
        )
        .with_sql("demo/GET/count.sql", "SELECT count(*) AS n FROM t")
        .build()
        .await;

    let (status, body) = app.request("POST", "/demo/tx-fail", Some("{}"), &[]).await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "BadSqlGrammarException");

    let (s2, cnt) = app.request("GET", "/demo/count", None, &[]).await;
    assert_eq!(s2, 200);
    assert_eq!(cnt[0]["n"], 0);
}

#[tokio::test]
async fn non_transactional_post_leaves_committed_writes() {
    // Control: without the marker, a two-statement POST where the second
    // statement fails still commits the first (baseline behaviour that
    // task 003 fixes only when the marker is present).
    let app = TestAppBuilder::new()
        .with_seed("CREATE TABLE t (id INTEGER);")
        .with_sql(
            "demo/POST/no-tx.sql",
            "INSERT INTO t (id) VALUES (1); \
             SELECT * FROM nonexistent_table",
        )
        .with_sql("demo/GET/count.sql", "SELECT count(*) AS n FROM t")
        .build()
        .await;

    let (status, _) = app.request("POST", "/demo/no-tx", Some("{}"), &[]).await;
    assert_eq!(status, 400);

    let (_s, cnt) = app.request("GET", "/demo/count", None, &[]).await;
    assert_eq!(cnt[0]["n"], 1); // First INSERT committed (no tx wrapping).
}

#[tokio::test]
async fn batch_body_must_have_queries_field() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/lookup.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, body) = app
        .request("POST", "/demo/lookup/batch", Some(r#"{"other":[]}"#), &[])
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "MalformedRequestException");
}

#[tokio::test]
async fn repeated_named_params_bind_correctly() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/repeat.sql", "SELECT :x AS a, :x AS b, :y AS c")
        .build()
        .await;
    let (status, body) = app
        .request("POST", "/demo/repeat", Some(r#"{"x":5, "y":9}"#), &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"a": 5, "b": 5, "c": 9}]));
}

#[tokio::test]
async fn case_insensitive_endpoint_lookup() {
    let app = TestAppBuilder::new()
        .with_sql("Demo/POST/MixedCase.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, body) = app
        .request("POST", "/DEMO/mixedcase", Some("{}"), &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"n": 1}]));
}

#[tokio::test]
async fn deeply_nested_paths_work() {
    let app = TestAppBuilder::new()
        .with_sql("demo/GET/a/b/c/deep.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, body) = app.request("GET", "/demo/a/b/c/deep", None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"n": 1}]));
}

#[tokio::test]
async fn method_mismatch_returns_query_not_found() {
    // Only POST is defined; GET the same path should 400 (not 405) to mirror
    // Java behaviour where the query key is method-specific.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, body) = app.request("GET", "/demo/x", None, &[]).await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "ResqlRuntimeException");
}
