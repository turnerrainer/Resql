//! F-RES-3 regression pins — optional inter-service bearer gate.
//!
//! The h2ck.me v1 runtime break-test flagged that any Resql that ever
//! becomes network-reachable outside its intended trust boundary is
//! one `curl` away from unauth SQL execution. This module proves the
//! opt-in gate closes that lane without breaking the default posture.

mod common;
use common::TestAppBuilder;

const TOKEN: &str = "s3cret-inter-service-token-32b-abc";

#[tokio::test]
async fn bearer_gate_off_by_default_allows_unauth_query() {
    // The gate is opt-in — no token configured means the pre-fix
    // posture is unchanged. Every existing integration test in the
    // suite depends on this.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, _body) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn bearer_gate_rejects_missing_authorization_header() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .inter_service_token(TOKEN)
        .build()
        .await;
    let (status, code, message, body) = app.request_err("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(status, 401);
    assert_eq!(code, "UnauthorizedException");
    // Deliberately generic — no "invalid token" vs "missing header"
    // oracle for the caller.
    assert_eq!(message, "Authentication required");
    // Envelope shape unchanged (issue #25).
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test]
async fn bearer_gate_rejects_wrong_token() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .inter_service_token(TOKEN)
        .build()
        .await;
    let (status, code, _msg, _body) = app
        .request_err(
            "POST",
            "/demo/x",
            Some("{}"),
            &[("authorization", "Bearer nope")],
        )
        .await;
    assert_eq!(status, 401);
    assert_eq!(code, "UnauthorizedException");
}

#[tokio::test]
async fn bearer_gate_accepts_correct_token() {
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .inter_service_token(TOKEN)
        .build()
        .await;
    let auth = format!("Bearer {TOKEN}");
    let (status, _body) = app
        .request(
            "POST",
            "/demo/x",
            Some("{}"),
            &[("authorization", auth.as_str())],
        )
        .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn bearer_gate_lets_health_probes_through() {
    // Load-balancer liveness / readiness must not require the token —
    // else every rolling deploy would flap the endpoint.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .inter_service_token(TOKEN)
        .build()
        .await;
    let (status, _body) = app.request("GET", "/health", None, &[]).await;
    assert_eq!(status, 200);
    let (status, _body) = app.request("GET", "/healthz", None, &[]).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn bearer_gate_rejects_wrong_scheme() {
    // `Basic <b64>` or `Token <str>` doesn't count — the middleware
    // only recognises `Bearer <token>`.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .inter_service_token(TOKEN)
        .build()
        .await;
    let (status, _code, _msg, _body) = app
        .request_err(
            "POST",
            "/demo/x",
            Some("{}"),
            &[("authorization", "Token s3cret-inter-service-token-32b-abc")],
        )
        .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn bearer_gate_rejects_prefix_or_extra_bytes() {
    // Constant-time compare + exact length match: neither a strict
    // prefix nor a longer superstring counts as valid.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .inter_service_token(TOKEN)
        .build()
        .await;
    let too_short = "Bearer s3cret-inter-service-token-32b-ab"; // 1 char short
    let too_long = format!("Bearer {TOKEN}extra");
    let (s1, _, _, _) = app
        .request_err(
            "POST",
            "/demo/x",
            Some("{}"),
            &[("authorization", too_short)],
        )
        .await;
    let (s2, _, _, _) = app
        .request_err(
            "POST",
            "/demo/x",
            Some("{}"),
            &[("authorization", too_long.as_str())],
        )
        .await;
    assert_eq!(s1, 401);
    assert_eq!(s2, 401);
}
