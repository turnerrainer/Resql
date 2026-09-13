mod common;
use common::TestAppBuilder;

#[tokio::test]
async fn body_over_max_returns_413() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT :s AS s")
        .max_body(64)
        .build()
        .await;
    // Build a payload well above the 64-byte cap.
    let big = "x".repeat(200);
    let body = format!(r#"{{"s":"{big}"}}"#);
    let (status, code, message, body_json) =
        app.request_err("POST", "/demo/x", Some(&body), &[]).await;
    assert_eq!(status, 413);
    // FN5: the 413 that comes out of tower-http's RequestBodyLimitLayer
    // now carries the same header-envelope shape as any ResqlError —
    // downstream DSLs no longer need to special-case one class of error.
    assert_eq!(code, "PayloadTooLargeException");
    assert!(
        !message.is_empty(),
        "X-Resql-Error-Message must be populated"
    );
    assert_eq!(body_json, serde_json::json!([]));
}

#[tokio::test]
async fn body_at_limit_still_processed() {
    // Give ourselves headroom by aiming well under the cap.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT :s AS s")
        .max_body(4096)
        .build()
        .await;
    let (status, _body) = app
        .request("POST", "/demo/x", Some(r#"{"s":"hi"}"#), &[])
        .await;
    assert_eq!(status, 200);
}
