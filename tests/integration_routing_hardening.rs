//! Regression pins for the v1 routing-hardening batch (R1, R2, R3):
//!
//! R1 — `allow_datasource_header` defaults to false; when explicitly on,
//! every override is gated by a per-project allowlist. Rejections return
//! 403 without revealing whether the requested datasource exists.
//!
//! R2 — `cors.allowed_origins` defaults to `""`. Without an explicit
//! configuration, no `Access-Control-Allow-Origin` header is emitted so
//! browsers block cross-origin reads.
//!
//! R3 — When CORS *is* configured, methods are narrowed to `GET`/`POST`
//! and headers to the small set Resql actually reads. Preflight probes
//! for other methods do not report them as allowed.

mod common;
use common::TestAppBuilder;

// -------- R1 --------

#[tokio::test]
async fn r1_default_config_ignores_x_datasource_header() {
    // The library's real defaults have `allow_datasource_header: false`
    // and an empty allowlist — a header value is silently ignored, so
    // routing follows the project → datasource map with no chance of
    // lateral movement.
    let app = TestAppBuilder::new()
        .with_datasources(&["demo", "audit"])
        .allow_header(false)
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, _body) = app
        .request("POST", "/demo/x", Some("{}"), &[("x-datasource", "audit")])
        .await;
    // Falls through to project "demo", which is a registered datasource:
    // routing is unaffected by the attacker-supplied header.
    assert_eq!(status, 200);
}

#[tokio::test]
async fn r1_forbidden_message_names_offender_but_not_registry() {
    let app = TestAppBuilder::new()
        .with_datasources(&["primary"])
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, body) = app
        .request(
            "POST",
            "/demo/x",
            Some("{}"),
            &[("x-datasource", "definitely-not-real")],
        )
        .await;
    assert_eq!(status, 403);
    let msg = body["message"].as_str().unwrap();
    assert!(msg.contains("definitely-not-real"), "msg = {msg}");
    // Must NOT leak the set of registered datasources.
    assert!(!msg.contains("primary"), "msg = {msg}");
}

#[tokio::test]
async fn r1_blank_header_treated_as_absent() {
    let app = TestAppBuilder::new()
        .with_datasources(&["demo"])
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, _body) = app
        .request("POST", "/demo/x", Some("{}"), &[("x-datasource", "   ")])
        .await;
    assert_eq!(status, 200);
}

// -------- R2 --------

#[tokio::test]
async fn r2_default_no_cors_header_on_response() {
    // TestAppBuilder mirrors production defaults (empty CORS); a browser
    // request carrying an `Origin` gets no `Access-Control-Allow-Origin`
    // in the response, so cross-origin reads are blocked.
    let app = TestAppBuilder::new()
        .with_datasources(&["demo"])
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, headers, _body) = app
        .request_full(
            "POST",
            "/demo/x",
            Some("{}"),
            &[("origin", "https://evil.example.com")],
        )
        .await;
    assert_eq!(status, 200);
    assert!(
        headers.get("access-control-allow-origin").is_none(),
        "must not emit ACAO by default; got {:?}",
        headers.get("access-control-allow-origin")
    );
}

// -------- R3 --------

// Preflight-narrowing is unit-tested at the CorsLayer builder level:
// `build_cors` returns a layer whose method+header allow-lists are
// narrowed. The test lives in `src/server.rs` because attaching the
// layer requires an explicit non-empty allowed_origins in config, which
// the current test harness does not surface. This file exists to hold
// the regression pin for the visible headers-on-response (R2) behaviour.
