//! R6 — the `request_timeout_seconds` config is now enforced by a
//! `TimeoutLayer`. Long-running queries no longer pin pool
//! connections indefinitely; the middleware aborts the future and
//! returns 408, and (on Postgres) `SET statement_timeout` kills the
//! server-side query too.
//!
//! T-16 — the same timeout covers slow-body ingest: a client that
//! streams a partial body and stalls does not get to occupy a
//! connection slot past `request_timeout_seconds`. The `TimeoutLayer`
//! sits around the handler future, and the body extractor runs
//! *inside* that future — so an incomplete stream is aborted the same
//! way a slow SQL query is.

mod common;
use common::TestAppBuilder;
use std::time::{Duration, Instant};

#[tokio::test]
async fn slow_query_hits_request_timeout() {
    // Recursive CTE that's guaranteed to burn many seconds of CPU —
    // easily overshoots the 1-second timeout the test config sets.
    let sql = r#"
        WITH RECURSIVE r(n) AS (
          SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 500000000
        )
        SELECT COUNT(*) AS c FROM r
    "#;
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/slow.sql", sql)
        .request_timeout_secs(1)
        .build()
        .await;

    let start = Instant::now();
    // Outer safety net: if the timeout wiring is broken, the recursive
    // CTE would potentially run for minutes. Cap the wait so the test
    // suite can't hang.
    let (status, code, _message, body) = tokio::time::timeout(
        Duration::from_secs(20),
        app.request_err("POST", "/demo/slow", Some("{}"), &[]),
    )
    .await
    .expect("request should complete within the outer safety timeout");
    let elapsed = start.elapsed();

    // TimeoutLayer surfaces the deadline as 408 Request Timeout. The
    // handler must fail — this test is only meaningful if the CTE
    // does *not* complete inside 1s.
    assert!(
        status == 408 || status == 504,
        "expected 408/504 timeout status, got {status} after {elapsed:?}"
    );
    // FN5: 504 from the tower-http TimeoutLayer now carries the
    // header-envelope shape, same as any other error path.
    assert_eq!(code, "RequestTimeoutException");
    assert_eq!(body, serde_json::json!([]));
    assert!(
        elapsed >= Duration::from_millis(900),
        "elapsed {elapsed:?} — timeout fired earlier than the configured 1s"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "elapsed {elapsed:?} — timeout took much longer than expected"
    );
}

// T-16 — slow-body ingest is covered by the same request timeout.
//
// This test constructs a request whose body is a stream that yields
// its first chunk and then stalls forever. Axum's Bytes extractor
// pulls the whole body before invoking the handler; that pull happens
// inside the handler future, which is wrapped by `TimeoutLayer`. So a
// half-sent body must produce 504 (RequestTimeoutException in the
// envelope), not a hung request pinning a pool slot.
#[tokio::test]
async fn slow_body_hits_request_timeout() {
    use axum::body::Body;
    use axum::http::Request;
    use futures_util::{stream, StreamExt};
    use tower::ServiceExt;

    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .request_timeout_secs(1)
        .build()
        .await;

    // Stream: yield an opening chunk, then sleep 30s (which the
    // 1-second TimeoutLayer will interrupt long before). `Body::from_stream`
    // takes any `Stream<Item = Result<impl Into<Bytes>, ...>>`, so
    // this stays independent of the specific hyper/bytes version pinned
    // by axum's transitive deps.
    let s = stream::iter(vec![Ok::<_, std::io::Error>(b"{".to_vec())]).chain(stream::once(
        async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok::<_, std::io::Error>(b"}".to_vec())
        },
    ));
    let body = Body::from_stream(s);

    let req = Request::builder()
        .method("POST")
        .uri("/demo/x")
        .header("content-type", "application/json")
        .body(body)
        .unwrap();

    let start = Instant::now();
    let resp = tokio::time::timeout(Duration::from_secs(10), app.router.clone().oneshot(req))
        .await
        .expect("router future should complete inside the safety net")
        .expect("oneshot should not error");
    let elapsed = start.elapsed();

    let status = resp.status().as_u16();
    let code = resp
        .headers()
        .get("x-resql-error-code")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // TimeoutLayer surfaces the deadline as 504. Anything else means
    // the slow-body reader wasn't inside the timeout scope.
    assert_eq!(
        status, 504,
        "expected 504 timeout, got {status} in {elapsed:?}"
    );
    assert_eq!(code, "RequestTimeoutException");
    assert!(
        elapsed >= Duration::from_millis(900),
        "timeout fired earlier than the configured 1s (elapsed {elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — slow-body wasn't aborted by the timeout"
    );
}
