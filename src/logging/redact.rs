//! Redaction of secret-bearing fields before values enter a log line.
//!
//! Two shapes:
//! - `redact_json` walks a JSON value and replaces any key that
//!   matches (case-insensitive) one of `field_names` with the
//!   sentinel `"[REDACTED]"`.
//! - `redact_headers` returns a header map with matched names' values
//!   replaced by the same sentinel.
//!
//! Neither mutates in place — cloning here is deliberate so the
//! request path stays untouched.

use serde_json::{Map, Value};
use std::collections::HashMap;

pub const REDACTED_SENTINEL: &str = "[REDACTED]";

/// Recursively redact matching field names in a JSON value. Case-
/// insensitive on the key; the *value* is replaced wholesale, so
/// nested secrets under a redacted key are collapsed too.
pub fn redact_json(v: &Value, field_names: &[String]) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (k, val) in map {
                if field_matches(k, field_names) {
                    out.insert(k.clone(), Value::String(REDACTED_SENTINEL.to_string()));
                } else {
                    out.insert(k.clone(), redact_json(val, field_names));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|i| redact_json(i, field_names)).collect())
        }
        other => other.clone(),
    }
}

/// Redact header values whose names match `redact_names` (case-
/// insensitive). Returns a fresh map — callers can safely log the
/// return value without touching the request's original headers.
pub fn redact_headers(
    headers: &HashMap<String, String>,
    redact_names: &[String],
) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(headers.len());
    for (k, v) in headers {
        if field_matches(k, redact_names) {
            out.insert(k.clone(), REDACTED_SENTINEL.to_string());
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn field_matches(name: &str, list: &[String]) -> bool {
    list.iter().any(|n| n.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_top_level() {
        let v = json!({ "user": "a", "password": "hunter2" });
        let out = redact_json(&v, &["password".into()]);
        assert_eq!(out, json!({ "user": "a", "password": "[REDACTED]" }));
    }

    #[test]
    fn redacts_case_insensitive() {
        let v = json!({ "Authorization": "Bearer xyz" });
        let out = redact_json(&v, &["authorization".into()]);
        assert_eq!(out, json!({ "Authorization": "[REDACTED]" }));
    }

    #[test]
    fn redacts_nested() {
        let v = json!({ "outer": { "token": "abc", "keep": 1 } });
        let out = redact_json(&v, &["token".into()]);
        assert_eq!(
            out,
            json!({ "outer": { "token": "[REDACTED]", "keep": 1 } })
        );
    }

    #[test]
    fn redacts_in_arrays() {
        let v = json!([{ "secret": "s1" }, { "secret": "s2" }]);
        let out = redact_json(&v, &["secret".into()]);
        assert_eq!(
            out,
            json!([{ "secret": "[REDACTED]" }, { "secret": "[REDACTED]" }])
        );
    }

    #[test]
    fn header_redaction() {
        let mut h = HashMap::new();
        h.insert("Authorization".to_string(), "Bearer xyz".to_string());
        h.insert("x-custom".to_string(), "keep".to_string());
        let out = redact_headers(&h, &["authorization".into()]);
        assert_eq!(out.get("Authorization").unwrap(), "[REDACTED]");
        assert_eq!(out.get("x-custom").unwrap(), "keep");
    }
}
