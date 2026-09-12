//! Fleet stronghold §5.1: the five browser-side defence headers must
//! appear on every response. Resql is a JSON API behind Ruuter, so a
//! browser should never render its output, but a proxy
//! misconfiguration that serves the JSON as `text/html` (or an
//! attacker who tricks a browser into re-typing it) would let a hostile
//! payload get evaluated. These headers close that lane cheaply.

mod common;
use common::TestAppBuilder;

/// Every one of these must be present on every response Resql produces.
const REQUIRED_HEADERS: &[&str] = &[
    "content-security-policy",
    "strict-transport-security",
    "x-frame-options",
    "x-content-type-options",
    "referrer-policy",
];

#[tokio::test]
async fn security_headers_present_on_health_probe() {
    let app = TestAppBuilder::new().build().await;
    let (status, headers, _body) = app.request_full("GET", "/health", None, &[]).await;
    assert_eq!(status, 200);
    for name in REQUIRED_HEADERS {
        assert!(
            headers.get(*name).is_some(),
            "missing security header: {name}"
        );
    }
}

#[tokio::test]
async fn security_headers_present_on_query_success() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, headers, _body) = app.request_full("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(status, 200);
    for name in REQUIRED_HEADERS {
        assert!(
            headers.get(*name).is_some(),
            "missing security header: {name}"
        );
    }
}

#[tokio::test]
async fn security_headers_present_on_error_response() {
    let app = TestAppBuilder::new().build().await;
    let (status, headers, _body) = app
        .request_full("POST", "/nope/nope", Some("{}"), &[])
        .await;
    assert_eq!(status, 400);
    for name in REQUIRED_HEADERS {
        assert!(
            headers.get(*name).is_some(),
            "missing security header on error response: {name}"
        );
    }
}

#[tokio::test]
async fn security_headers_exact_expected_values() {
    let app = TestAppBuilder::new().build().await;
    let (_status, headers, _body) = app.request_full("GET", "/health", None, &[]).await;
    assert_eq!(
        headers
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap(),
        "default-src 'none'; frame-ancestors 'none'"
    );
    assert_eq!(
        headers
            .get("strict-transport-security")
            .unwrap()
            .to_str()
            .unwrap(),
        "max-age=63072000; includeSubDomains"
    );
    assert_eq!(
        headers.get("x-frame-options").unwrap().to_str().unwrap(),
        "DENY"
    );
    assert_eq!(
        headers
            .get("x-content-type-options")
            .unwrap()
            .to_str()
            .unwrap(),
        "nosniff"
    );
    assert_eq!(
        headers.get("referrer-policy").unwrap().to_str().unwrap(),
        "no-referrer"
    );
}
