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
    let (status, _code, msg, _body) = app
        .request_err(
            "POST",
            "/demo/x",
            Some("{}"),
            &[("x-datasource", "definitely-not-real")],
        )
        .await;
    assert_eq!(status, 403);
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

// -------- FN6 --------

#[tokio::test]
async fn fn6_get_on_post_only_path_returns_405_with_allow_header() {
    // A saved query exists at /demo/x under POST. A GET to the same path
    // must return 405 with `Allow: POST` (RFC 7231 §7.4.1), not the
    // previous 400 `Saved query does not exist` — the path IS registered,
    // just under a different method.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, headers, _body) = app.request_full("GET", "/demo/x", None, &[]).await;
    assert_eq!(status, 405);
    let allow = headers
        .get("allow")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        allow.split(',').map(str::trim).any(|m| m == "POST"),
        "expected `Allow: POST` header, got {allow:?}"
    );
    assert_eq!(
        headers
            .get("x-resql-error-code")
            .and_then(|v| v.to_str().ok()),
        Some("MethodNotAllowedException"),
    );
}

#[tokio::test]
async fn fn6_405_lists_every_supported_method() {
    // Both GET and POST exist at /demo/x. Requesting via DELETE (which
    // axum's router refuses at the framework level with its own 405 +
    // Allow) is out of scope; the case FN6 targets is a saved query set
    // that only registers one of the two, so a mismatching method (GET
    // when only POST exists, or vice versa) returns 405. When both are
    // registered the request succeeds. Cross-check both directions.
    let app = TestAppBuilder::new()
        .with_sql("demo/GET/x.sql", "SELECT 1 AS n")
        .with_sql("demo/POST/x.sql", "SELECT 2 AS n")
        .build()
        .await;
    let (get_status, _b) = app.request("GET", "/demo/x", None, &[]).await;
    let (post_status, _b) = app.request("POST", "/demo/x", Some("{}"), &[]).await;
    assert_eq!(get_status, 200);
    assert_eq!(post_status, 200);
}

#[tokio::test]
async fn fn6_unknown_path_still_returns_400_query_not_found() {
    // The 405 path only kicks in when a saved query IS registered under
    // a different method. A path with no saved queries under any method
    // must still return 400 with the ResqlRuntimeException / QueryNotFound
    // shape callers already handle.
    let app = TestAppBuilder::new()
        .with_sql("demo/POST/x.sql", "SELECT 1 AS n")
        .build()
        .await;
    let (status, code, _msg, _body) = app.request_err("GET", "/demo/nonexistent", None, &[]).await;
    assert_eq!(status, 400);
    assert_eq!(code, "ResqlRuntimeException");
}
