mod common;
use common::TestAppBuilder;

#[tokio::test]
async fn health_returns_up_with_all_java_fields() {
    let app = TestAppBuilder::new().build().await;
    let (status, body) = app.request("GET", "/health", None, &[]).await;
    assert_eq!(status, 200);
    // The four Java-canonical fields (DTOHeartBeatInfo) all present, plus
    // Rust's additive `status` field.
    assert_eq!(body["appName"], "resql");
    // Version format: v{MAJOR}.{MINOR}.{PATCH} matching Java's
    // HeartBeatService.getVersion (v prefix + dotted semver core).
    assert!(
        body["version"].as_str().unwrap().starts_with('v'),
        "version = {}",
        body["version"]
    );
    // packagingTime present as a number (may be 0 when RESQL_BUILD_TIME
    // was not set at build time). Java clients (JSONAssert LENIENT) only
    // check the field exists — matching that here.
    assert!(body["packagingTime"].is_number());
    assert!(body["appStartTime"].as_u64().unwrap() > 0);
    assert!(body["serverTime"].as_u64().unwrap() > 0);
    // Rust-only addition, safe for Java clients that ignore unknown fields.
    assert_eq!(body["status"], "UP");
}

#[tokio::test]
async fn healthz_alias_returns_up() {
    let app = TestAppBuilder::new().build().await;
    let (status, body) = app.request("GET", "/healthz", None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "UP");
}

#[tokio::test]
async fn datasources_endpoint_returns_java_shape() {
    let app = TestAppBuilder::new()
        .with_datasources(&["one", "two"])
        .build()
        .await;
    let (status, body) = app.request("GET", "/datasources", None, &[]).await;
    assert_eq!(status, 200);
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let names: Vec<&str> = arr.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"one"));
    assert!(names.contains(&"two"));
    for entry in arr {
        // Java's DataSourceConfigProperties shape: name, jdbcUrl, username,
        // driverClassName (password stripped by @JsonIgnore).
        assert!(entry.get("name").is_some(), "missing `name`: {entry}");
        assert!(entry.get("jdbcUrl").is_some(), "missing `jdbcUrl`: {entry}");
        assert!(
            entry.get("username").is_some(),
            "missing `username`: {entry}"
        );
        assert_eq!(entry["driverClassName"], "org.sqlite.JDBC");
        // Password never emitted.
        let s = entry.to_string();
        assert!(!s.contains("password"), "leaked password field: {s}");
        // Old Rust field names (from before compat pass) must not be present
        // — they'd break Java-scoped dashboards.
        assert!(
            entry.get("url").is_none(),
            "unexpected `url` alongside `jdbcUrl`: {entry}"
        );
        assert!(
            entry.get("driver").is_none(),
            "unexpected `driver` alongside `driverClassName`: {entry}"
        );
    }
}
