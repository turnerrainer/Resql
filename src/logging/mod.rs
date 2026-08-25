//! Structured-logging helpers used by the axum middleware and the
//! error emission path. Field vocabulary follows the OpenTelemetry
//! HTTP semantic conventions; utilities cover CRLF sanitization,
//! error-chain rendering, W3C traceparent parsing / generation, and
//! JSON body redaction / capping.
//!
//! Reference: `book/src/logging.md` (operator-facing chapter).

pub mod redact;

use crate::config::LoggingConfig;

/// Strip CR / LF from a value about to enter a log field. Prevents
/// log-line splicing when an attacker-controlled header or body
/// field carries a newline. Cheap on the common case (no newline →
/// same allocation).
pub fn sanitize_log_value(s: &str) -> String {
    if !s.contains('\n') && !s.contains('\r') {
        return s.to_string();
    }
    s.replace(['\n', '\r'], " ")
}

/// Format an error's `source()` chain as ` -> caused by: <msg>` links,
/// bounded to 5 hops so a runaway cause chain can't fill a log line.
pub fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = String::new();
    let mut src = err.source();
    let mut hops = 0;
    while let Some(e) = src {
        if hops >= 5 {
            out.push_str(" -> caused by: ...");
            break;
        }
        out.push_str(" -> caused by: ");
        out.push_str(&sanitize_log_value(&e.to_string()));
        src = e.source();
        hops += 1;
    }
    out
}

/// Extract the 32-hex trace id from a W3C `traceparent` value, or
/// `None` if the input isn't well-formed. Cheap; used by the request
/// middleware to decorate every request-scoped log line with
/// `trace_id=…`.
pub fn trace_id_from_traceparent(tp: &str) -> Option<&str> {
    let parts: Vec<&str> = tp.splitn(4, '-').collect();
    if parts.len() == 4 && parts[1].len() == 32 {
        Some(parts[1])
    } else {
        None
    }
}

/// Generate a fresh W3C `traceparent` header value with a random
/// 128-bit trace id and 64-bit span id, `01` sampled. Used when the
/// caller didn't send a `traceparent` so every request-scoped log
/// line still carries a stable `trace_id`.
pub fn generate_traceparent() -> String {
    let trace_hex = format!("{:032x}", uuid::Uuid::new_v4().as_u128());
    let span_hex = format!("{:016x}", (uuid::Uuid::new_v4().as_u128() as u64));
    format!("00-{trace_hex}-{span_hex}-01")
}

/// Render a JSON value for a log line, redacting configured field
/// names at any depth and capping the serialised length. Returns
/// `"-"` when nothing to render.
pub fn render_body_for_log(value: Option<&serde_json::Value>, cfg: &LoggingConfig) -> String {
    match value {
        Some(v) => {
            let redacted = redact::redact_json(v, &cfg.redact_body_fields);
            let s = serde_json::to_string(&redacted).unwrap_or_else(|_| "-".to_string());
            cap_and_sanitize(&s, cfg.max_body_bytes)
        }
        None => "-".to_string(),
    }
}

/// Cap the string at `max_bytes` (cuts at char boundary) and
/// sanitize CR/LF. Appends `…` when truncated.
pub fn cap_and_sanitize(s: &str, max_bytes: usize) -> String {
    let sanitized = sanitize_log_value(s);
    if sanitized.len() <= max_bytes {
        return sanitized;
    }
    let mut end = max_bytes;
    while end > 0 && !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 3);
    out.push_str(&sanitized[..end]);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_stripped_from_log_value() {
        assert_eq!(sanitize_log_value("a\nb\rc"), "a b c");
        assert_eq!(sanitize_log_value("plain"), "plain");
    }

    #[test]
    fn trace_id_extract() {
        assert_eq!(
            trace_id_from_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(trace_id_from_traceparent("garbage"), None);
        assert_eq!(trace_id_from_traceparent("00-short-x-01"), None);
    }

    #[test]
    fn generated_traceparent_round_trips_through_extractor() {
        let tp = generate_traceparent();
        let tid = trace_id_from_traceparent(&tp).expect("extractable");
        assert_eq!(tid.len(), 32);
        assert!(tid.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn error_chain_bounded() {
        use std::error::Error;
        use std::fmt;
        #[derive(Debug)]
        struct E {
            msg: &'static str,
            source: Option<Box<E>>,
        }
        impl fmt::Display for E {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{}", self.msg)
            }
        }
        impl Error for E {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                self.source.as_deref().map(|e| e as &(dyn Error + 'static))
            }
        }
        let deep = (0..10).fold(
            E {
                msg: "leaf",
                source: None,
            },
            |acc, _| E {
                msg: "wrap",
                source: Some(Box::new(acc)),
            },
        );
        let chain = error_chain(&deep);
        assert!(chain.contains("caused by"));
        assert!(chain.ends_with("caused by: ..."));
    }

    #[test]
    fn cap_body_ok_shorter() {
        assert_eq!(cap_and_sanitize("hi", 100), "hi");
    }

    #[test]
    fn cap_body_truncates() {
        let out = cap_and_sanitize(&"x".repeat(50), 10);
        assert_eq!(out.len(), 10 + '…'.len_utf8());
        assert!(out.ends_with('…'));
    }
}
