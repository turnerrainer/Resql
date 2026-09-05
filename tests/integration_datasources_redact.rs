//! R5 — `/datasources` no longer leaks hostnames or usernames, and
//! defaults to 404 (indistinguishable from a non-mounted endpoint) so
//! unauth callers can't fingerprint the service as Resql either.

mod common;
use common::TestAppBuilder;

#[tokio::test]
async fn datasources_endpoint_returns_404_by_default() {
    let app = TestAppBuilder::new()
        .with_datasources(&["crm"])
        .datasources_public(false)
        .build()
        .await;
    let (status, _body) = app.request("GET", "/datasources", None, &[]).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn datasources_endpoint_omits_username_and_redacts_url() {
    let app = TestAppBuilder::new()
        .with_datasources(&["crm"])
        .datasources_public(true)
        .build()
        .await;
    let (status, body) = app.request("GET", "/datasources", None, &[]).await;
    assert_eq!(status, 200);
    let entry = &body[0];
    // Field is retained for backward-compat JSON shape but must be blank.
    assert_eq!(entry["username"], "");
    // URL should not contain a path (`::memory:` etc.), userinfo, or
    // query — just the scheme portion.
    let jdbc = entry["jdbcUrl"].as_str().unwrap();
    assert!(jdbc.starts_with("sqlite"), "jdbc = {jdbc}");
    assert!(!jdbc.contains("memory"), "jdbc must not leak SQLite path detail: {jdbc}");
    assert!(!jdbc.contains('@'), "jdbc must not carry userinfo: {jdbc}");
    // Password never present.
    assert!(entry.get("password").is_none());
}
