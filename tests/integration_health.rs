mod common;
use common::TestAppBuilder;

#[tokio::test]
async fn health_returns_up() {
    let app = TestAppBuilder::new().build().await;
    let (status, body) = app.request("GET", "/health", None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "UP");
    assert_eq!(body["appName"], "resql-on-rust");
    assert!(body["version"].as_str().unwrap().starts_with('0'));
    assert!(body["appStartTime"].as_u64().unwrap() > 0);
    assert!(body["serverTime"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn healthz_alias_returns_up() {
    let app = TestAppBuilder::new().build().await;
    let (status, body) = app.request("GET", "/healthz", None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "UP");
}

#[tokio::test]
async fn datasources_endpoint_lists_configured() {
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
        assert_eq!(entry["driver"], "sqlite");
        let s = entry.to_string();
        assert!(!s.contains("password"), "leaked password field: {s}");
    }
}
