//! Mandatory per-file declaration block (task 008).
//!
//! Every `.sql` file must open with a `/* … */` block-comment
//! declaration whose body is plain YAML — no per-line prefix, so
//! authors can copy-paste YAML from any editor without reformatting:
//!
//! ```sql
//! /*
//! description: Find a user by login, optionally filtered by status.
//! params:
//!   login:  { type: string, required: true }
//!   status: { type: string, required: false }
//! returns:
//!   - { name: id,    type: integer }
//!   - { name: email, type: string  }
//! */
//! SELECT id, email FROM users WHERE login = :login;
//! ```
//!
//! Rules — enforced at boot; runtime never sees a partial declaration:
//!
//! - The block comment must be the first non-whitespace content in
//!   the file.
//! - Content between `/*` and `*/` is fed verbatim to serde_yaml_ng.
//! - `params:` is required (may be empty `{}`).
//! - Every `:name` referenced by the SQL must appear in `params`.
//! - Every entry in `params` must be referenced by at least one
//!   `:name` in the SQL (typo protection).
//! - `deny_unknown_fields` on every struct — a typo in a param
//!   attribute fails the boot instead of silently no-op'ing.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

use crate::error::ResqlError;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    pub params: HashMap<String, DeclaredParam>,
    #[serde(default)]
    pub returns: Option<Vec<ReturnField>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredParam {
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub items: Option<Box<ItemType>>,
    /// Closed set of permitted values (JSON Schema `enum`). Bounds the
    /// input space at the request boundary: values outside the set are
    /// rejected before any SQL binding. Each entry must match the
    /// declared `type` — enforced at boot, no drift.
    #[serde(rename = "enum", default)]
    pub allowed: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemType {
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(default)]
    pub format: Option<String>,
    /// Nested element type for arrays-of-arrays. Postgres arrays are
    /// physically flat (they're multi-dimensional but homogeneously
    /// typed), so any nested-array param binds as JSONB regardless;
    /// declaring this here doesn't change the wire shape, it turns on
    /// **recursive per-element validation** so a mistyped grand-child
    /// element (`[[1, "two"]]` where `items: {type: array, items:
    /// {type: integer}}` was declared) fails at the request boundary
    /// with a path like `xs[0][1]` instead of silently being shoved
    /// into JSONB.
    #[serde(default)]
    pub items: Option<Box<ItemType>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
    Date,
    Datetime,
    Uuid,
}

impl ParamType {
    /// JSON Schema type keyword for OpenAPI emission.
    pub fn openapi_type(self) -> &'static str {
        match self {
            ParamType::String | ParamType::Date | ParamType::Datetime | ParamType::Uuid => "string",
            ParamType::Integer => "integer",
            ParamType::Number => "number",
            ParamType::Boolean => "boolean",
            ParamType::Array => "array",
            ParamType::Object => "object",
        }
    }

    /// Standard OpenAPI `format` for the semantic types. Callers still
    /// honour an explicit `format:` override on the declaration.
    pub fn openapi_format(self) -> Option<&'static str> {
        match self {
            ParamType::Integer => Some("int64"),
            ParamType::Number => Some("double"),
            ParamType::Date => Some("date"),
            ParamType::Datetime => Some("date-time"),
            ParamType::Uuid => Some("uuid"),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ParamType::String => "string",
            ParamType::Integer => "integer",
            ParamType::Number => "number",
            ParamType::Boolean => "boolean",
            ParamType::Array => "array",
            ParamType::Object => "object",
            ParamType::Date => "date",
            ParamType::Datetime => "datetime",
            ParamType::Uuid => "uuid",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_nullable")]
    pub nullable: bool,
}

fn default_nullable() -> bool {
    true
}

/// Parse a SQL file's leading `/* … */` block comment as YAML,
/// returning (declaration, remaining SQL). Errors when the block
/// comment is missing, unterminated, or contains malformed YAML.
///
/// Leading whitespace before the block comment is tolerated so
/// authors can leave a blank line at the top of the file.
pub fn parse(sql: &str) -> Result<(Declaration, String), String> {
    let (yaml_text, remaining) = extract_block(sql)?;
    let decl: Declaration = serde_yaml_ng::from_str(&yaml_text)
        .map_err(|e| format!("declaration YAML is invalid: {e}"))?;
    validate_items_placement(&decl)?;
    validate_enum_shapes(&decl)?;
    Ok((decl, remaining))
}

/// `items:` is meaningful only on `type: array`. Declaring it on a
/// scalar type is nonsense — silently ignored at runtime, which
/// swallows the author's intent. Fail-fast so a typo like
/// `type: string, items: {type: integer}` surfaces at boot.
fn validate_items_placement(decl: &Declaration) -> Result<(), String> {
    for (name, spec) in &decl.params {
        if spec.items.is_some() && spec.ty != ParamType::Array {
            return Err(format!(
                "param `{name}`: `items` is only valid on `type: array` \
                 (declared type is `{}`)",
                spec.ty.as_str()
            ));
        }
    }
    Ok(())
}

/// Boot-time sanity check on any `enum:` declared for a param:
/// - list must be non-empty (empty `enum` accepts nothing — unusable)
/// - every entry must match the declared `type`
/// - if a `default:` is declared, it must be in the enum set (or null)
///
/// Fail-fast at load so bad shape can never reach the request path.
fn validate_enum_shapes(decl: &Declaration) -> Result<(), String> {
    for (name, spec) in &decl.params {
        let Some(values) = &spec.allowed else {
            continue;
        };
        if values.is_empty() {
            return Err(format!("param `{name}`: `enum` list must not be empty"));
        }
        for v in values {
            if !value_matches_type(spec.ty, v) {
                return Err(format!(
                    "param `{name}`: enum value {v} does not match declared type `{}`",
                    spec.ty.as_str()
                ));
            }
        }
        if let Some(default) = &spec.default {
            if !default.is_null() && !values.iter().any(|v| v == default) {
                return Err(format!(
                    "param `{name}`: default {default} is not in the declared enum set"
                ));
            }
        }
    }
    Ok(())
}

fn value_matches_type(ty: ParamType, v: &Value) -> bool {
    match (ty, v) {
        (_, Value::Null) => true,
        (
            ParamType::String | ParamType::Date | ParamType::Datetime | ParamType::Uuid,
            Value::String(_),
        ) => true,
        (ParamType::Integer, Value::Number(n)) => {
            n.is_i64() || n.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false)
        }
        (ParamType::Number, Value::Number(_)) => true,
        (ParamType::Boolean, Value::Bool(_)) => true,
        _ => false,
    }
}

/// Locate the leading `/* … */` block comment and split into
/// (YAML body, remainder). YAML body is the exact text between the
/// opening `/*` and closing `*/` — pasted verbatim, no per-line
/// prefix to strip.
fn extract_block(sql: &str) -> Result<(String, String), String> {
    // Skip leading whitespace.
    let trimmed_start = sql
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)
        .ok_or_else(|| "file is empty".to_string())?;
    let rest = &sql[trimmed_start..];
    if !rest.starts_with("/*") {
        return Err("file must open with a `/* … */` block-comment declaration".into());
    }
    let body_start = trimmed_start + 2;
    let after_open = &sql[body_start..];
    let close_rel = after_open
        .find("*/")
        .ok_or_else(|| "declaration block comment `/*` at top of file is not closed".to_string())?;
    let yaml_text = &sql[body_start..body_start + close_rel];
    let remaining = sql[body_start + close_rel + 2..].to_string();
    Ok((yaml_text.to_string(), remaining))
}

/// Validate the declaration against the parameter names actually
/// referenced by the SQL. Returns Err with a specific reason if the
/// two sides disagree — the declaration MUST cover every `:name` and
/// MUST NOT list any name that never appears (typo protection).
pub fn validate_against_sql(decl: &Declaration, referenced: &[String]) -> Result<(), String> {
    for name in referenced {
        if !decl.params.contains_key(name) {
            return Err(format!(
                "SQL references parameter `:{name}` but declaration has no `params.{name}`"
            ));
        }
    }
    for name in decl.params.keys() {
        if !referenced.iter().any(|r| r == name) {
            return Err(format!(
                "declaration lists param `{name}` but no `:{name}` appears in the SQL"
            ));
        }
    }
    Ok(())
}

/// Load-time-only helper: given a serde_yaml_ng parse failure, produce
/// a `ResqlError::InvalidDeclaration` pointing at the file.
pub fn invalid<P: Into<std::path::PathBuf>>(path: P, reason: String) -> ResqlError {
    ResqlError::InvalidDeclaration {
        path: path.into(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_declaration_and_returns_remaining_sql() {
        let src = "/*\nparams: {}\n*/\nSELECT 1;\n";
        let (decl, rest) = parse(src).unwrap();
        assert!(decl.params.is_empty());
        assert_eq!(rest.trim(), "SELECT 1;");
    }

    #[test]
    fn skips_leading_whitespace() {
        let src = "\n\n/*\nparams: {}\n*/\nSELECT 1;\n";
        let (decl, _) = parse(src).unwrap();
        assert!(decl.params.is_empty());
    }

    #[test]
    fn rejects_missing_block_comment() {
        let src = "SELECT 1;\n";
        assert!(parse(src).is_err());
    }

    #[test]
    fn rejects_unterminated_block_comment() {
        let src = "/*\nparams: {}\nSELECT 1;\n";
        assert!(parse(src).is_err());
    }

    #[test]
    fn parses_full_shape() {
        let src = r#"/*
description: Find a user
namespace: crm
params:
  login: { type: string, required: true }
  status: { type: string, required: false, default: null }
returns:
  - { name: id,    type: integer }
  - { name: email, type: string, nullable: false }
*/
SELECT id, email FROM users WHERE login = :login;
"#;
        let (decl, rest) = parse(src).unwrap();
        assert_eq!(decl.description.as_deref(), Some("Find a user"));
        assert_eq!(decl.namespace.as_deref(), Some("crm"));
        assert_eq!(decl.params.len(), 2);
        assert_eq!(decl.params["login"].ty, ParamType::String);
        assert!(decl.params["login"].required);
        assert!(!decl.params["status"].required);
        let returns = decl.returns.as_ref().unwrap();
        assert_eq!(returns.len(), 2);
        assert_eq!(returns[1].name, "email");
        assert!(!returns[1].nullable);
        assert!(rest.contains("SELECT"));
    }

    #[test]
    fn rejects_unknown_field_on_param() {
        let src = "/*\nparams:\n  x: { type: string, requird: true }\n*/\nSELECT :x;\n";
        assert!(parse(src).is_err());
    }

    #[test]
    fn rejects_unknown_type() {
        let src = "/*\nparams:\n  x: { type: blob }\n*/\nSELECT :x;\n";
        assert!(parse(src).is_err());
    }

    #[test]
    fn validate_flags_missing_param() {
        let decl: Declaration =
            serde_yaml_ng::from_str("params:\n  a: { type: string }\n").unwrap();
        let err = validate_against_sql(&decl, &["a".into(), "b".into()]).unwrap_err();
        assert!(err.contains("`:b`"), "err = {err}");
    }

    #[test]
    fn validate_flags_unreferenced_declared_param() {
        let decl: Declaration =
            serde_yaml_ng::from_str("params:\n  a: { type: string }\n  b: { type: string }\n")
                .unwrap();
        let err = validate_against_sql(&decl, &["a".into()]).unwrap_err();
        assert!(err.contains("`b`"), "err = {err}");
    }

    #[test]
    fn parses_enum_on_string_param() {
        let src = "/*\nparams:\n  status: { type: string, required: true, enum: [active, disabled] }\n*/\nSELECT :status;\n";
        let (decl, _) = parse(src).unwrap();
        let allowed = decl.params["status"].allowed.as_ref().unwrap();
        assert_eq!(allowed.len(), 2);
        assert_eq!(allowed[0], serde_json::json!("active"));
    }

    #[test]
    fn rejects_enum_value_of_wrong_type() {
        // Integer values on a string-declared param must fail at boot.
        let src = "/*\nparams:\n  status: { type: string, enum: [1, 2] }\n*/\nSELECT :status;\n";
        let err = parse(src).unwrap_err();
        assert!(err.contains("does not match declared type"), "err = {err}");
    }

    #[test]
    fn rejects_empty_enum_list() {
        let src = "/*\nparams:\n  status: { type: string, enum: [] }\n*/\nSELECT :status;\n";
        let err = parse(src).unwrap_err();
        assert!(err.contains("must not be empty"), "err = {err}");
    }

    #[test]
    fn rejects_default_outside_enum_set() {
        let src = "/*\nparams:\n  status: { type: string, default: foo, enum: [active, disabled] }\n*/\nSELECT :status;\n";
        let err = parse(src).unwrap_err();
        assert!(err.contains("not in the declared enum set"), "err = {err}");
    }

    #[test]
    fn accepts_null_default_with_enum() {
        // JSON Schema semantics: null bypasses enum. A nullable optional
        // with a declared enum + `default: null` must load.
        let src = "/*\nparams:\n  status: { type: string, default: null, enum: [active, disabled] }\n*/\nSELECT :status;\n";
        parse(src).unwrap();
    }

    #[test]
    fn rejects_items_on_scalar_type() {
        // `items: {...}` next to `type: string` is a typo, not a
        // valid shape. Silent-ignore would swallow the author's
        // intent — fail at boot instead.
        let src = "/*\nparams:\n  x: { type: string, items: { type: integer } }\n*/\nSELECT :x;\n";
        let err = parse(src).unwrap_err();
        assert!(
            err.contains("`items` is only valid on `type: array`"),
            "err = {err}"
        );
    }

    #[test]
    fn accepts_items_on_array_type() {
        // Sanity: the reverse case (correct placement) still loads.
        let src = "/*\nparams:\n  xs: { type: array, items: { type: integer } }\n*/\nSELECT :xs;\n";
        parse(src).unwrap();
    }

    #[test]
    fn openapi_type_maps_semantic_types_to_string_with_format() {
        assert_eq!(ParamType::Date.openapi_type(), "string");
        assert_eq!(ParamType::Date.openapi_format(), Some("date"));
        assert_eq!(ParamType::Uuid.openapi_type(), "string");
        assert_eq!(ParamType::Uuid.openapi_format(), Some("uuid"));
        assert_eq!(ParamType::Integer.openapi_format(), Some("int64"));
    }
}
