//! R8 regression pins — optional global rate limiter.
//!
//! `security.rate_limit.requests_per_second: 0` (default) disables the
//! middleware entirely — every existing test in the suite depends on
//! that. Non-zero enables a global token bucket sized by the
//! `burst` field (default: 2 * rps).

mod common;
use common::TestAppBuilder;

#[tokio::test]
async fn rate_limit_disabled_by_default_allows_burst() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    for _ in 0..20 {
        let (status, _) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
        assert_eq!(status, 200);
    }
}

#[tokio::test]
async fn rate_limit_returns_429_after_burst_exhausted() {
    // rps=1, burst=1 → the first request consumes the only token; the
    // second (within the same second) fails.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .rate_limit(1, 1)
        .build()
        .await;
    let (first, _) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(first, 200);
    let (second, headers, body) = app.request_full("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(second, 429, "expected rate-limit rejection, body={body}");
    assert!(
        headers.get("retry-after").is_some(),
        "Retry-After header required by RFC 6585"
    );
    // Envelope shape (issue #25) applies to 429 too — body is `[]` +
    // canonical error headers.
    assert_eq!(body, serde_json::json!([]));
    assert_eq!(
        headers
            .get("x-resql-error-code")
            .and_then(|v| v.to_str().ok()),
        Some("TooManyRequestsException"),
    );
}

#[tokio::test]
async fn rate_limit_bypasses_health_probes() {
    // Even at rps=1, health probes never consume tokens — LB liveness
    // must not depend on the rate budget.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .rate_limit(1, 1)
        .build()
        .await;
    for _ in 0..50 {
        let (status, _) = app.request("GET", "/health", None, &[]).await;
        assert_eq!(status, 200);
        let (status, _) = app.request("GET", "/healthz", None, &[]).await;
        assert_eq!(status, 200);
    }
}

#[tokio::test]
async fn rate_limit_refills_over_time() {
    // rps=10, burst=1: after exhausting the single token, wait for
    // one full refill window and confirm the next request succeeds.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .rate_limit(10, 1)
        .build()
        .await;
    let (a, _) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
    let (b, _) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(a, 200);
    assert_eq!(b, 429);
    // At 10 rps a token refills in ~100 ms; sleep well past that.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let (c, _) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(c, 200, "bucket must have refilled after 250 ms");
}
