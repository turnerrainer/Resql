mod common;
use common::TestAppBuilder;
use serde_json::json;

#[tokio::test]
async fn x_datasource_header_routes_to_named_datasource() {
    let app = TestAppBuilder::new()
        .with_datasources(&["primary", "secondary"])
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    // datasource named "demo" does NOT exist; header names "primary".
    let (status, body) = app
        .request(
            "POST",
            "/demo/x",
            Some("{}"),
            &[("x-datasource", "primary")],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"n": 1}]));
}

#[tokio::test]
async fn x_datasource_header_with_unknown_name_is_forbidden() {
    // Post-R1 semantics: an unrecognised value in the X-Datasource
    // header is rejected by the per-project allowlist BEFORE Resql looks
    // the name up in the datasource registry. Attackers can no longer
    // enumerate which datasources exist by sending guesses.
    let app = TestAppBuilder::new()
        .with_datasources(&["primary"])
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, code, message, body) = app
        .request_err("POST", "/demo/x", Some("{}"), &[("x-datasource", "nope")])
        .await;
    assert_eq!(status, 403);
    assert_eq!(code, "ForbiddenDatasourceOverrideException");
    assert!(message.contains("'nope'"), "message = {message}");
    // Body invariant per #25: always `[]` on any query-endpoint error.
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test]
async fn x_datasource_header_ignored_when_disabled() {
    let app = TestAppBuilder::new()
        .with_datasources(&["demo", "other"])
        .allow_header(false)
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    // header would route to "other" if allowed, but with_datasources_header
    // is off so it falls back to the project name "demo".
    let (status, body) = app
        .request("POST", "/demo/x", Some("{}"), &[("x-datasource", "other")])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"n": 1}]));
}

#[tokio::test]
async fn project_map_routes_when_no_header() {
    let app = TestAppBuilder::new()
        .with_datasources(&["primary"])
        .map_project("legacyapp", "primary")
        .with_sql("legacyapp/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, body) = app.request("POST", "/legacyapp/x", Some("{}"), &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!([{"n": 1}]));
}

#[tokio::test]
async fn project_defaulted_to_project_name_when_unmapped() {
    let app = TestAppBuilder::new()
        .with_datasources(&["demo"])
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, _body) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(status, 200);
}
