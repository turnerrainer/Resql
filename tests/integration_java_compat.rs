//! Regression tests for Java-config compatibility (REFACTO-REQUIREMENTS
//! §2.1, §2.3, §2.4, §4.4). Each test would have FAILED before the compat
//! shim landed. They deliberately try to break the fix rather than confirm
//! it, per the CLAUDE.md "audit-cycle lessons" note.

use resql::config::Config;
use resql::config_compat::DiagLevel;

/// A stripped-down copy of the Java `src/main/resources/application.yml`
/// from Resql/src/main/resources/application.yml (source of truth). Loading
/// this must succeed on the Rust target with the compat shim active.
const JAVA_DEV_APPLICATION_YML: &str = r#"
spring:
  profiles:
    active: dev

server:
  port: 8082

headers:
  contentSecurityPolicy: "script-src 'self'"

userIPHeaderName: x-forwarded-for
userIPLoggingPrefix: from IP
userIPLoggingMDCkey: userIP

h2:
  console:
    enabled: true

sqlms:
  saved-queries-dir: "./templates/"
  datasources:
    - name: test_db_1
      jdbcUrl: "sqlite::memory:"
      username: h2
      password: h2
      driverClassName: org.h2.Driver
logging:
  level:
    root: info
"#;

#[test]
fn java_application_yml_loads_successfully() {
    // Sets the env var the compat shim would normally require if the operator
    // migrated to password_env. Here we exercise the plaintext-password path.
    let cfg = Config::from_yaml_str(JAVA_DEV_APPLICATION_YML)
        .expect("Java-shape application.yml must load");
    assert_eq!(cfg.server.bind, "0.0.0.0:8082", "server.port → server.bind");
    assert_eq!(cfg.sql_dir.to_str(), Some("./templates/"));
    assert_eq!(cfg.datasources.len(), 1);
    let ds = &cfg.datasources[0];
    assert_eq!(ds.name, "test_db_1");
    assert_eq!(ds.url, "sqlite::memory:", "jdbcUrl → url");
    assert_eq!(ds.username, "h2");
    assert_eq!(ds.password.as_deref(), Some("h2"), "plaintext accepted");
}

#[test]
fn java_application_yml_emits_diagnostics_naming_java_only_fields() {
    let cfg = Config::from_yaml_str(JAVA_DEV_APPLICATION_YML).unwrap();
    let field_names: Vec<&str> = cfg
        .compat_diagnostics
        .iter()
        .map(|d| d.source_field.as_str())
        .collect();
    // §6.2: operator must see, at boot, every Java-only field that has no
    // wired-up effect in the Rust target.
    for expected in [
        "spring",
        "userIPHeaderName",
        "userIPLoggingPrefix",
        "userIPLoggingMDCkey",
        "h2",
        "headers.contentSecurityPolicy",
    ] {
        assert!(
            field_names.contains(&expected),
            "no diagnostic for `{expected}`; got {field_names:?}"
        );
    }
    // The plaintext password diagnostic MUST be WARN level (security posture
    // downgrade should not be a silent INFO).
    let pw = cfg
        .compat_diagnostics
        .iter()
        .find(|d| d.source_field.contains("password"))
        .expect("plaintext password diagnostic missing");
    assert!(
        matches!(pw.level, DiagLevel::Warn),
        "plaintext password must be WARN, got {:?}",
        pw.level
    );
}

#[test]
fn default_sql_dir_matches_java_default_when_unset() {
    // Java's `sqlms.saved-queries-dir` defaults to `./templates/`. A Java
    // operator with no explicit config value must not fail-to-boot on Rust.
    let cfg = Config::from_yaml_str("datasources: []\n").unwrap();
    assert_eq!(cfg.sql_dir.to_str(), Some("./templates/"));
}

#[test]
fn sqlms_prefix_takes_no_priority_over_top_level_if_both_present() {
    // Belt-and-braces: an operator who runs a MIXED config (Rust top-level
    // wins over Java sqlms.* nested) must see a WARN so the intent is not
    // lost silently.
    let cfg = Config::from_yaml_str(
        r#"
sql_dir: ./rust-preferred
sqlms:
  saved-queries-dir: ./java-legacy
"#,
    )
    .unwrap();
    assert_eq!(cfg.sql_dir.to_str(), Some("./rust-preferred"));
    // Diagnostic exists (either the sqlms.saved-queries-dir naming or the
    // collision warning). Either way the operator has boot-log evidence.
    assert!(
        !cfg.compat_diagnostics.is_empty(),
        "mixed config must produce a diagnostic"
    );
}

#[test]
fn jdbc_url_alias_accepted_and_username_preserved() {
    // Regression: R2.4 (field-name aliases). Java operators reference
    // `jdbcUrl`; the Rust target's canonical field is `url`. Both must work.
    let cfg = Config::from_yaml_str(
        r#"
sql_dir: ./sql
datasources:
  - name: primary
    jdbcUrl: "sqlite::memory:"
    username: alice
    password: secret
"#,
    )
    .unwrap();
    let ds = &cfg.datasources[0];
    assert_eq!(ds.url, "sqlite::memory:");
    assert_eq!(ds.username, "alice");
    assert_eq!(ds.password.as_deref(), Some("secret"));
}

#[test]
fn driver_class_name_accepted_and_ignored() {
    // R2.4 + §2.1 clause 3: `driverClassName` from Java configs must be
    // accepted; the operator sees a diagnostic naming it.
    let cfg = Config::from_yaml_str(
        r#"
sql_dir: ./sql
datasources:
  - name: primary
    url: "sqlite::memory:"
    driverClassName: org.h2.Driver
"#,
    )
    .unwrap();
    assert!(cfg
        .compat_diagnostics
        .iter()
        .any(|d| d.source_field.contains("driverClassName")));
}

#[test]
fn plaintext_and_env_password_both_set_is_rejected() {
    // §6.1 hygiene: an operator ambiguously specifying both shapes should be
    // rejected at parse time with a clear message, not silently pick one.
    std::env::set_var("__RESQL_COMPAT_TEST_PW", "x");
    let err = Config::from_yaml_str(
        r#"
sql_dir: ./sql
datasources:
  - name: primary
    url: "sqlite::memory:"
    username: u
    password: plain
    password_env: __RESQL_COMPAT_TEST_PW
"#,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("BOTH") || msg.contains("password"),
        "err = {msg}"
    );
    std::env::remove_var("__RESQL_COMPAT_TEST_PW");
}

#[test]
fn typo_in_unknown_top_level_key_still_rejected() {
    // R2.2 must still fire even with the compat shim active — unknown keys
    // stay a hard error unless they're in the recognised Java-legacy set.
    let err = Config::from_yaml_str("sql_dir: ./sql\ncomplete_typo_here: 1\n").unwrap_err();
    assert!(err
        .to_string()
        .to_lowercase()
        .contains("invalid config yaml"));
}

#[test]
fn logging_level_map_translates_to_envfilter_directive() {
    // Java logs config uses a per-logger map. Rust's `logging.level` is a
    // single EnvFilter directive. The shim must translate — otherwise the
    // operator's config fails deserialisation.
    let cfg = Config::from_yaml_str(
        r#"
sql_dir: ./sql
logging:
  level:
    root: info
    rig.sqlms: debug
"#,
    )
    .unwrap();
    // rig.sqlms=debug is the Java pattern; Rust uses `resql=debug` internally
    // but the shim only translates format, not per-logger names.
    assert!(
        cfg.logging.level.starts_with("info"),
        "got {}",
        cfg.logging.level
    );
    assert!(
        cfg.logging.level.contains("rig.sqlms=debug"),
        "got {}",
        cfg.logging.level
    );
}

#[test]
fn empty_password_is_treated_as_no_password() {
    // A Java YAML with `password: ""` should not crash; behave the same as
    // omitting the field.
    let cfg = Config::from_yaml_str(
        r#"
sql_dir: ./sql
datasources:
  - name: primary
    url: "sqlite::memory:"
    password: ""
"#,
    )
    .unwrap();
    let ds = &cfg.datasources[0];
    // Either None or Some("") is acceptable, but resolved_url must not
    // interpolate an empty password.
    let url = ds.resolved_url().unwrap();
    assert_eq!(url, "sqlite::memory:");
}
