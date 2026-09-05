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

#[tokio::test]
async fn error_body_shape_matches_java_reference() {
    let expected = load_reference("error-body-java-shape.json");
    let expected_obj = expected.as_object().unwrap();

    let app = TestAppBuilder::new().build().await;
    let (status, body) = app
        .request("POST", "/nonexistent-project/nonexistent", Some("{}"), &[])
        .await;
    // Java-known conditions always return 400 with the Java shape.
    assert_eq!(status, 400);
    for (key, _) in expected_obj {
        if key.starts_with('_') {
            continue;
        }
        assert!(
            body.get(key).is_some(),
            "error body missing Java-canonical key `{key}`; body = {body}"
        );
    }
    // Exception class matches Java's SimpleName.
    assert_eq!(body["error"], "ResqlRuntimeException");
    // Message contains the requested path (Java: "Saved query '%s' does not exist").
    assert!(
        body["message"].as_str().unwrap().contains("nonexistent"),
        "message = {}",
        body["message"]
    );
}
