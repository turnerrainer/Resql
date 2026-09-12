//! R6 — the `request_timeout_seconds` config is now enforced by a
//! `TimeoutLayer`. Long-running queries no longer pin pool
//! connections indefinitely; the middleware aborts the future and
//! returns 408, and (on Postgres) `SET statement_timeout` kills the
//! server-side query too.

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
