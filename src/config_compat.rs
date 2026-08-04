//! Java Resql configuration compatibility shim.
//!
//! The Java Spring Boot Resql accepts a wildly different YAML shape than the
//! Rust target's canonical `resql.yaml`. Per the user's directive that the
//! Rust reimplementation "may not end up being incompatible to the source",
//! this module preprocesses a raw YAML document into the Rust canonical shape
//! and emits diagnostics for anything Java-specific it saw.
//!
//! Guarantees:
//! - Rust-canonical YAML passes through unchanged (aside from re-serialisation).
//! - Java-shape YAML is translated field-by-field into Rust shape.
//! - Anything Java-specific but not translatable (e.g. `spring.profiles.active`,
//!   `headers.contentSecurityPolicy`) is recorded as a diagnostic so the boot
//!   log can name the field and point to the migration (REFACTO-REQUIREMENTS
//!   §2.1 clause 3 / §6.2).
//!
//! The preprocessor is deliberately conservative: it only rewrites keys it
//! recognises. Unknown top-level keys still trip `deny_unknown_fields` in the
//! downstream `Config` struct, which is the R2.2 posture we want.

use serde_yaml_ng::{Mapping, Value};

/// A single boot-time diagnostic captured from Java-shape config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: DiagLevel,
    /// Java-shape field name (as authored, e.g. `sqlms.saved-queries-dir`).
    pub source_field: String,
    /// Message the operator sees at boot.
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagLevel {
    Info,
    Warn,
}

/// Preprocess a raw YAML document into the Rust canonical shape.
///
/// Returns the rewritten YAML (still a `Value`, ready for `serde_yaml_ng`
/// deserialisation) alongside the diagnostics that were surfaced. The caller
/// is expected to emit the diagnostics through `tracing` once the logging
/// subsystem is initialised.
pub fn preprocess(raw: Value) -> (Value, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let value = match raw {
        Value::Mapping(m) => Value::Mapping(rewrite_top_level(m, &mut diags)),
        other => other,
    };
    (value, diags)
}

fn rewrite_top_level(mut m: Mapping, diags: &mut Vec<Diagnostic>) -> Mapping {
    // 1. Flatten `sqlms:` prefix — Java's Spring @ConfigurationProperties root.
    //    `sqlms.saved-queries-dir` → `saved-queries-dir` at top level (later
    //    rewritten to `sql_dir` by step 2).
    //    `sqlms.datasources` → `datasources` at top level.
    //
    // Collision handling: for each `sqlms.KEY`, we check whether ANY of the
    // "would-be-Rust-canonical" candidates already exists at top level (e.g.
    // `saved-queries-dir` collides with a top-level `sql_dir`, `savedQueriesDir`,
    // OR another `saved-queries-dir`). If so, emit a WARN naming the
    // conflict; drop the nested value.
    if let Some(Value::Mapping(sqlms)) = m.remove("sqlms") {
        for (k, v) in sqlms {
            let name = match &k {
                Value::String(s) => s.clone(),
                _ => {
                    // Non-string keys in sqlms map are extraordinarily
                    // unlikely; forward verbatim and let downstream complain.
                    if !m.contains_key(&k) {
                        m.insert(k, v);
                    }
                    continue;
                }
            };
            let clash = top_level_aliases(&name)
                .iter()
                .any(|alt| m.contains_key(Value::String((*alt).to_string())));
            if clash {
                diags.push(Diagnostic {
                    level: DiagLevel::Warn,
                    source_field: format!("sqlms.{name}"),
                    message: format!(
                        "both `sqlms.{name}` and a top-level equivalent are present; using top-level, dropping `sqlms.{name}`"
                    ),
                });
                continue;
            }
            m.insert(Value::String(name), v);
        }
    }

    // 2. `saved-queries-dir` / `savedQueriesDir` → `sql_dir` (Java default field name).
    for alias in ["saved-queries-dir", "savedQueriesDir"] {
        if let Some(v) = m.remove(alias) {
            if !m.contains_key("sql_dir") {
                m.insert(Value::String("sql_dir".into()), v);
            }
        }
    }

    // 3. `server.port` (integer, Spring) → `server.bind` (host:port, Rust).
    //    Also unwrap Spring's `server.address` if present.
    if let Some(Value::Mapping(sm)) = m.get_mut("server") {
        let bind_present = sm.contains_key("bind");
        let port = sm.remove("port");
        let address = sm.remove("address");
        if !bind_present {
            let host = match address {
                Some(Value::String(s)) => s,
                _ => "0.0.0.0".to_string(),
            };
            let port_str = match port {
                Some(Value::Number(n)) => Some(n.to_string()),
                Some(Value::String(s)) => Some(s),
                _ => None,
            };
            if let Some(p) = port_str {
                sm.insert(
                    Value::String("bind".into()),
                    Value::String(format!("{host}:{p}")),
                );
            }
        } else if port.is_some() {
            diags.push(Diagnostic {
                level: DiagLevel::Warn,
                source_field: "server.port".into(),
                message: "both `server.port` and `server.bind` set; using `server.bind`".into(),
            });
        }
    }

    // 4. Datasources: normalize per-item field names.
    if let Some(Value::Sequence(seq)) = m.get_mut("datasources") {
        for (i, item) in seq.iter_mut().enumerate() {
            if let Value::Mapping(dsm) = item {
                rewrite_datasource(dsm, i, diags);
            }
        }
    }

    // 5. `logging.level.root: X` (Spring shape) → `logging.level: X` (Rust shape).
    if let Some(Value::Mapping(logging)) = m.get_mut("logging") {
        if let Some(Value::Mapping(level_map)) = logging.get("level").cloned() {
            // Java lets `logging.level` be a map keyed by logger name. Rust's
            // level is a single EnvFilter directive. We translate:
            //   { root: info, rig.sqlms: debug } → "info,rig.sqlms=debug"
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in level_map {
                let key = match k {
                    Value::String(s) => s,
                    _ => continue,
                };
                let lvl = match v {
                    Value::String(s) => s.to_lowercase(),
                    _ => continue,
                };
                if key == "root" {
                    parts.insert(0, lvl);
                } else {
                    parts.push(format!("{key}={lvl}"));
                }
            }
            if !parts.is_empty() {
                logging.insert(
                    Value::String("level".into()),
                    Value::String(parts.join(",")),
                );
            }
        }
    }

    // 6. `cors.allowedOrigins` → `cors.allowed_origins`.
    if let Some(Value::Mapping(cors)) = m.get_mut("cors") {
        if let Some(v) = cors.remove("allowedOrigins") {
            if !cors.contains_key("allowed_origins") {
                cors.insert(Value::String("allowed_origins".into()), v);
            }
        }
    }

    // 7. `headers.contentSecurityPolicy` — Java security header. Rust does not
    //    have a first-class CSP config yet; record as an unmigrated field.
    if let Some(Value::Mapping(headers)) = m.remove("headers") {
        for (k, _) in headers {
            if let Value::String(name) = k {
                diags.push(Diagnostic {
                    level: DiagLevel::Warn,
                    source_field: format!("headers.{name}"),
                    message: format!(
                        "`headers.{name}` (Java) has no target equivalent yet; header will not be emitted. See DIVERGENCES.md"
                    ),
                });
            }
        }
    }

    // 8. Spring-only top-level keys — accept and warn, do not fail.
    for spring_key in ["spring", "h2", "management", "info"] {
        if m.remove(spring_key).is_some() {
            diags.push(Diagnostic {
                level: DiagLevel::Info,
                source_field: spring_key.into(),
                message: format!(
                    "`{spring_key}` block is Spring-specific and has no effect on the Rust target"
                ),
            });
        }
    }

    // 9. User-IP MDC/logging keys — Java-side observability that Rust doesn't wire.
    for k in [
        "userIPHeaderName",
        "userIPLoggingPrefix",
        "userIPLoggingMDCkey",
    ] {
        if m.remove(k).is_some() {
            diags.push(Diagnostic {
                level: DiagLevel::Warn,
                source_field: k.into(),
                message: format!(
                    "`{k}` (Java) has no target equivalent yet; user-IP will not be logged. See DIVERGENCES.md"
                ),
            });
        }
    }

    m
}

fn rewrite_datasource(dsm: &mut Mapping, idx: usize, diags: &mut Vec<Diagnostic>) {
    // jdbcUrl / jdbc_url / jdbc-url → url
    for alias in ["jdbcUrl", "jdbc_url", "jdbc-url"] {
        if let Some(v) = dsm.remove(alias) {
            if !dsm.contains_key("url") {
                dsm.insert(Value::String("url".into()), v);
            }
        }
    }

    // driverClassName / driver_class_name / driver-class-name — accepted for
    // compat with Java configs, ignored by Rust (driver is derived from URL
    // scheme). Emit a diagnostic naming the field so the operator knows.
    let ds_id = ds_identity(dsm, idx);
    for alias in ["driverClassName", "driver_class_name", "driver-class-name"] {
        if let Some(Value::String(dc)) = dsm.remove(alias) {
            diags.push(Diagnostic {
                level: DiagLevel::Info,
                source_field: format!("datasources[{ds_id}].{alias}"),
                message: format!(
                    "`{alias}: {dc}` accepted for compat; driver is derived from URL scheme in the Rust target"
                ),
            });
        }
    }

    // Java's plaintext `password:` — accepted, but emit a WARN because the
    // Rust target prefers `password_env:` (env-var indirection) as its
    // security posture. See DIVERGENCES.md.
    if dsm.contains_key("password") && !dsm.contains_key("password_env") {
        diags.push(Diagnostic {
            level: DiagLevel::Warn,
            source_field: format!("datasources[{ds_id}].password"),
            message: format!(
                "datasource '{ds_id}' uses plaintext `password:` (Java shape). \
                 The Rust target prefers `password_env: <ENV_VAR_NAME>`. \
                 Plaintext accepted for compatibility. See DIVERGENCES.md."
            ),
        });
    }
}

fn ds_identity(dsm: &Mapping, idx: usize) -> String {
    match dsm.get("name") {
        Some(Value::String(s)) => s.clone(),
        _ => format!("#{idx}"),
    }
}

/// The set of top-level field names that would be considered equivalent to
/// `name`, so the flattener can catch collisions before dropping a value.
/// The set includes `name` itself, plus its Rust-canonical form and any
/// other Java aliases that map to the same target field.
fn top_level_aliases(name: &str) -> &'static [&'static str] {
    match name {
        "saved-queries-dir" | "savedQueriesDir" | "sql_dir" => {
            &["saved-queries-dir", "savedQueriesDir", "sql_dir"]
        }
        "datasources" => &["datasources"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml_ng::from_str;

    fn preprocess_str(s: &str) -> (Value, Vec<Diagnostic>) {
        preprocess(from_str::<Value>(s).unwrap())
    }

    #[test]
    fn passes_rust_canonical_yaml_unchanged() {
        let (v, d) = preprocess_str(
            "sql_dir: ./sql\ndatasources:\n  - name: x\n    url: \"sqlite::memory:\"\n",
        );
        assert!(d.is_empty(), "unexpected diagnostics: {d:?}");
        let m = v.as_mapping().unwrap();
        assert!(m.contains_key("sql_dir"));
        assert!(!m.contains_key("sqlms"));
    }

    #[test]
    fn flattens_sqlms_prefix() {
        let (v, _) = preprocess_str(
            r#"sqlms:
  saved-queries-dir: ./templates/
  datasources:
    - name: byk
      jdbcUrl: jdbc:postgresql://host/db
      username: u
      password: p
      driverClassName: org.postgresql.Driver
"#,
        );
        let m = v.as_mapping().unwrap();
        assert!(!m.contains_key("sqlms"));
        assert_eq!(
            m.get("sql_dir").and_then(Value::as_str),
            Some("./templates/")
        );
        let ds = m
            .get("datasources")
            .and_then(Value::as_sequence)
            .unwrap()
            .first()
            .unwrap()
            .as_mapping()
            .unwrap();
        assert_eq!(
            ds.get("url").and_then(Value::as_str),
            Some("jdbc:postgresql://host/db")
        );
        assert!(!ds.contains_key("jdbcUrl"));
        assert!(!ds.contains_key("driverClassName"));
    }

    #[test]
    fn diagnostic_for_plaintext_password() {
        let (_, d) = preprocess_str(
            r#"sql_dir: ./sql
datasources:
  - name: primary
    url: "sqlite::memory:"
    password: hunter2
"#,
        );
        assert!(d
            .iter()
            .any(|x| x.source_field.contains("password") && matches!(x.level, DiagLevel::Warn)));
    }

    #[test]
    fn diagnostic_for_spring_active_profile() {
        let (_, d) = preprocess_str("sql_dir: ./sql\nspring:\n  profiles:\n    active: dev\n");
        assert!(d.iter().any(|x| x.source_field == "spring"));
    }

    #[test]
    fn diagnostic_for_userip_header_name() {
        let (_, d) = preprocess_str("sql_dir: ./sql\nuserIPHeaderName: x-forwarded-for\n");
        assert!(d
            .iter()
            .any(|x| x.source_field == "userIPHeaderName" && matches!(x.level, DiagLevel::Warn)));
    }

    #[test]
    fn diagnostic_for_headers_csp() {
        let (_, d) = preprocess_str(
            "sql_dir: ./sql\nheaders:\n  contentSecurityPolicy: \"script-src 'self'\"\n",
        );
        assert!(d
            .iter()
            .any(|x| x.source_field == "headers.contentSecurityPolicy"));
    }

    #[test]
    fn server_port_converts_to_bind() {
        let (v, _) = preprocess_str("sql_dir: ./sql\nserver:\n  port: 8082\n");
        let bind = v
            .as_mapping()
            .and_then(|m| m.get("server"))
            .and_then(Value::as_mapping)
            .and_then(|s| s.get("bind"))
            .and_then(Value::as_str);
        assert_eq!(bind, Some("0.0.0.0:8082"));
    }

    #[test]
    fn server_port_and_address_combine() {
        let (v, _) =
            preprocess_str("sql_dir: ./sql\nserver:\n  address: 127.0.0.1\n  port: 8082\n");
        let bind = v
            .as_mapping()
            .and_then(|m| m.get("server"))
            .and_then(Value::as_mapping)
            .and_then(|s| s.get("bind"))
            .and_then(Value::as_str);
        assert_eq!(bind, Some("127.0.0.1:8082"));
    }

    #[test]
    fn logging_level_map_flattens_to_envfilter() {
        let (v, _) = preprocess_str(
            r#"sql_dir: ./sql
logging:
  level:
    root: info
    rig.sqlms: debug
"#,
        );
        let level = v
            .as_mapping()
            .and_then(|m| m.get("logging"))
            .and_then(Value::as_mapping)
            .and_then(|l| l.get("level"))
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        // root comes first, others appended with `=` in EnvFilter shape.
        assert!(level.starts_with("info"));
        assert!(level.contains("rig.sqlms=debug"));
    }

    #[test]
    fn cors_allowed_origins_camel_renamed() {
        let (v, _) = preprocess_str("sql_dir: ./sql\ncors:\n  allowedOrigins: https://a.example\n");
        let allowed = v
            .as_mapping()
            .and_then(|m| m.get("cors"))
            .and_then(Value::as_mapping)
            .and_then(|c| c.get("allowed_origins"))
            .and_then(Value::as_str);
        assert_eq!(allowed, Some("https://a.example"));
    }

    #[test]
    fn saved_queries_dir_alias_becomes_sql_dir() {
        let (v, _) = preprocess_str("saved-queries-dir: ./templates/\n");
        let sd = v
            .as_mapping()
            .and_then(|m| m.get("sql_dir"))
            .and_then(Value::as_str);
        assert_eq!(sd, Some("./templates/"));
    }
}
