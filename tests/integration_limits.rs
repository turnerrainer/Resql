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
    let (status, _body) = app.request("POST", "/demo/x", Some(&body), &[]).await;
    assert_eq!(status, 413);
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
