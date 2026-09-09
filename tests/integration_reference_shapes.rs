//! REFACTO-REQUIREMENTS §4.3: for each subsystem the target claims to
//! preserve, at least one end-to-end fixture must run against both source
//! and target. The full cross-implementation harness lives in
//! `compat/README.md` (manual). This suite is the automated half: it loads
//! the reference response shapes from `compat/reference-outputs/*.json`
//! and asserts the Rust target produces field-equivalent responses.

use serde_json::Value;
use std::path::Path;

mod common;
use common::TestAppBuilder;

fn load_reference(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("compat")
        .join("reference-outputs")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("reference file is not valid JSON")
}

#[tokio::test]
async fn healthz_response_keys_match_java_reference() {
    let expected = load_reference("healthz-java-shape.json");
    let expected_obj = expected.as_object().unwrap();

    let app = TestAppBuilder::new().build().await;
    let (status, body) = app.request("GET", "/healthz", None, &[]).await;
    assert_eq!(status, 200);

    // Every non-comment key in the Java reference MUST be present in the
    // Rust response with a semantically-equivalent value type.
    for (key, expected_value) in expected_obj {
        if key.starts_with('_') {
            continue;
        }
        let got = body.get(key).unwrap_or_else(|| {
            panic!("Rust /healthz missing Java-canonical key `{key}`; body = {body}")
        });
        assert_eq!(
            expected_value.is_number(),
            got.is_number(),
            "key `{key}`: expected number/{expected_value}, got {got}"
        );
        assert_eq!(
            expected_value.is_string(),
            got.is_string(),
            "key `{key}`: expected string/{expected_value}, got {got}"
        );
    }
    // Documented additive divergence — see DIV-002.
    assert_eq!(body["status"], "UP");
}

#[tokio::test]
async fn datasources_response_keys_match_java_reference() {
    let expected_arr = load_reference("datasources-java-shape.json");
    let expected_entry = expected_arr[0].as_object().unwrap();

    let app = TestAppBuilder::new()
        .with_datasources(&["crm"])
        // Post-R5 default is that `/datasources` returns 404 unless the
        // operator opts in. This shape test exists to lock the Java-
        // canonical field names when the endpoint IS mounted; flip the
        // flag on so the endpoint responds.
        .datasources_public(true)
        .build()
        .await;
    let (status, body) = app.request("GET", "/datasources", None, &[]).await;
    assert_eq!(status, 200);

    let entry = &body[0];
    for (key, expected_value) in expected_entry {
        if key.starts_with('_') {
            continue;
        }
        let got = entry.get(key).unwrap_or_else(|| {
            panic!("Rust /datasources missing Java-canonical key `{key}`; body = {entry}")
        });
        assert_eq!(
            expected_value.is_string(),
            got.is_string(),
            "key `{key}`: type drift"
        );
    }
    // Password never present (§/datasources contract).
    assert!(entry.get("password").is_none());
}

/// Documented divergence from the Java reference shape (issue #25).
/// Java Resql returns `{ "error": "...", "message": "..." }` in the
/// body on error. The Rust target returns an empty array body and
/// carries the same information in `X-Resql-Error-Code` and
/// `X-Resql-Error-Message` response headers so that a naive downstream
/// check like `body.length > 0` cannot silently fail-open into
/// "empty result" when the query actually errored.
#[tokio::test]
async fn error_envelope_delivered_via_headers_body_is_empty_array() {
    let app = TestAppBuilder::new().build().await;
    let (status, code, message, body) = app
        .request_err("POST", "/nonexistent-project/nonexistent", Some("{}"), &[])
        .await;
    // Java-known conditions still return 400.
    assert_eq!(status, 400);
    // Body is always `[]` on any query-endpoint error (#25).
    assert_eq!(body, serde_json::json!([]));
    // Exception class matches Java's SimpleName — now in the header.
    assert_eq!(code, "ResqlRuntimeException");
    // Message contains the requested path (Java: "Saved query '%s' does not exist").
    assert!(message.contains("nonexistent"), "message = {message}");
}

/// End-to-end check that the naive downstream DSL pattern from issue
/// #25 no longer silently fails open. The pattern is:
///
/// ```yaml
/// - condition: ${resql_res.response.body.length > 0}
///   next: found
///   next: not_found
/// ```
///
/// On a DB error the body must be an array with `length == 0` (routes
/// to `not_found`, which is expected), not a non-array object whose
/// `.length` is `undefined` (which historically evaluated to false and
/// silently routed to `not_found` on ANY error — turning DB errors into
/// wrong-404s or worse).
#[tokio::test]
async fn error_body_length_check_is_dsl_safe() {
    let app = TestAppBuilder::new().build().await;
    let (_status, _code, _msg, body) = app
        .request_err("POST", "/nonexistent-project/nonexistent", Some("{}"), &[])
        .await;
    let arr = body
        .as_array()
        .expect("error body must be an array so `.length` is defined");
    assert_eq!(arr.len(), 0, "body must be `[]` on error, got: {body}");
}
