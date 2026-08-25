//! OpenAPI 3.1 spec generator (task 008).
//!
//! Walks the loaded `QueryIndex` and emits an OpenAPI document at
//! boot; the document is cached on `AppState` and served at
//! `GET /openapi.json`.
//!
//! Rules:
//!
//! - **Path** = `/<project>/<name...>` — matches routing.
//! - **operationId** = `<method>_<project>_<slug>` where non-ident
//!   chars in the path suffix are replaced with `_`.
//! - **summary / description** = from `declaration.description` when
//!   set; otherwise summary is the filename stem and description is a
//!   short auto-generated note.
//! - **tags** = `[declaration.namespace | project]`.
//! - **parameters** (GET) = one query parameter per declared param,
//!   with the JSON Schema derived from `type` (+ `format`, `default`,
//!   `description`). `required` reflects the declaration.
//! - **requestBody** (POST) = JSON object schema built from
//!   `params`, with a `required` array of names marked required in the
//!   declaration. Batch endpoints get an extra `POST
//!   /<path>/batch` wrapping `{ queries: [<same shape>] }`.
//! - **responses** = 200 with `type: array` of the row shape derived
//!   from `returns` (or open-shape when absent); 400 → `Error`
//!   schema; 413 → same schema.
//!
//! Deterministic ordering: projects, methods, paths, all sorted alpha
//! at emit time so `diff` on the spec is meaningful across restarts.

use crate::config::OpenApiConfig;
use crate::declaration::{Declaration, DeclaredParam, ParamType, ReturnField};
use crate::loader::{HttpMethod, QueryIndex, SavedQuery};
use serde_json::{json, Map, Value};

pub const OPENAPI_VERSION: &str = "3.1.0";

/// Build a full OpenAPI 3.1 document from the loaded QueryIndex.
pub fn build_spec(index: &QueryIndex, cfg: &OpenApiConfig) -> Value {
    let mut items: Vec<&SavedQuery> = index.iter().collect();
    items.sort_by(|a, b| {
        a.project
            .cmp(&b.project)
            .then_with(|| a.method.as_str().cmp(b.method.as_str()))
            .then_with(|| a.path.cmp(&b.path))
    });

    let mut paths: Map<String, Value> = Map::new();
    for saved in items {
        let http_path = format!("/{}/{}", saved.project, saved.path);
        let op = build_operation(saved);
        let entry = paths
            .entry(http_path.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(map) = entry {
            map.insert(method_key(saved.method).to_string(), op);
        }
        // Batch variant for POST endpoints: same request body wrapped
        // in `{ queries: [...] }`, response is an array of arrays.
        if saved.method == HttpMethod::Post {
            let batch_path = format!("{http_path}/batch");
            let batch_op = build_batch_operation(saved);
            let entry = paths
                .entry(batch_path)
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(map) = entry {
                map.insert("post".into(), batch_op);
            }
        }
    }

    json!({
        "openapi": OPENAPI_VERSION,
        "info": {
            "title": cfg.title,
            "version": env!("CARGO_PKG_VERSION"),
            "description": cfg.description,
            "license": {
                "name": "Apache-2.0",
                "identifier": "Apache-2.0"
            }
        },
        "servers": [
            {"url": cfg.server_url, "description": "Resql instance"}
        ],
        "paths": Value::Object(paths),
        "components": {
            "schemas": {
                "Error": {
                    "type": "object",
                    "properties": {
                        "error": {"type": "string", "description": "Java-canonical exception class name"},
                        "message": {"type": "string"}
                    },
                    "required": ["error", "message"]
                }
            }
        }
    })
}

fn method_key(m: HttpMethod) -> &'static str {
    match m {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
    }
}

fn build_operation(saved: &SavedQuery) -> Value {
    let decl = &saved.declaration;
    let mut op = Map::new();
    op.insert(
        "operationId".into(),
        Value::String(operation_id(&saved.project, saved.method, &saved.path)),
    );
    op.insert("summary".into(), Value::String(summary(saved)));
    op.insert("description".into(), Value::String(description(saved)));
    op.insert(
        "tags".into(),
        json!([decl
            .namespace
            .clone()
            .unwrap_or_else(|| saved.project.clone())]),
    );

    match saved.method {
        HttpMethod::Get => {
            if let Some(params) = build_query_parameters(decl) {
                op.insert("parameters".into(), params);
            }
        }
        HttpMethod::Post => {
            op.insert("requestBody".into(), build_request_body(decl));
        }
    }
    op.insert("responses".into(), build_responses(decl, false));
    Value::Object(op)
}

fn build_batch_operation(saved: &SavedQuery) -> Value {
    let decl = &saved.declaration;
    let mut op = Map::new();
    op.insert(
        "operationId".into(),
        Value::String(format!(
            "{}_batch",
            operation_id(&saved.project, saved.method, &saved.path)
        )),
    );
    op.insert(
        "summary".into(),
        Value::String(format!("{} (atomic batch)", summary(saved))),
    );
    op.insert(
        "description".into(),
        Value::String(format!(
            "Atomic batch variant of `POST /{}/{}`. Body wraps a list of parameter \
             objects under `queries`; the whole batch runs in one transaction \
             and rolls back on any failure.",
            saved.project, saved.path
        )),
    );
    op.insert(
        "tags".into(),
        json!([decl
            .namespace
            .clone()
            .unwrap_or_else(|| saved.project.clone())]),
    );

    let single_schema = params_schema(decl);
    op.insert(
        "requestBody".into(),
        json!({
            "required": true,
            "content": {
                "application/json": {
                    "schema": {
                        "type": "object",
                        "required": ["queries"],
                        "properties": {
                            "queries": {
                                "type": "array",
                                "items": single_schema
                            }
                        }
                    }
                }
            }
        }),
    );
    op.insert("responses".into(), build_responses(decl, true));
    Value::Object(op)
}

fn operation_id(project: &str, method: HttpMethod, path: &str) -> String {
    let slug: String = path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{}_{}_{}", method_key(method), project, slug)
}

fn summary(saved: &SavedQuery) -> String {
    if let Some(d) = &saved.declaration.description {
        if let Some(first) = d.lines().next() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    saved
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&saved.path)
        .to_string()
}

fn description(saved: &SavedQuery) -> String {
    saved
        .declaration
        .description
        .clone()
        .unwrap_or_else(|| format!("Executes `{}`.", saved.source_file.display()))
}

fn build_query_parameters(decl: &Declaration) -> Option<Value> {
    let mut names: Vec<&String> = decl.params.keys().collect();
    names.sort();
    if names.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(names.len());
    for n in names {
        let spec = &decl.params[n];
        out.push(json!({
            "name": n,
            "in": "query",
            "required": spec.required,
            "description": spec.description.clone().unwrap_or_default(),
            "schema": param_schema(spec)
        }));
    }
    Some(Value::Array(out))
}

fn build_request_body(decl: &Declaration) -> Value {
    json!({
        "required": true,
        "content": {
            "application/json": {
                "schema": params_schema(decl)
            }
        }
    })
}

/// JSON Schema for the "single request body" shape: an object with one
/// property per declared param, `required` listing the required names.
fn params_schema(decl: &Declaration) -> Value {
    let mut names: Vec<&String> = decl.params.keys().collect();
    names.sort();
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();
    for n in &names {
        let spec = &decl.params[*n];
        properties.insert(n.to_string(), param_schema(spec));
        if spec.required {
            required.push(Value::String(n.to_string()));
        }
    }
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties
    });
    if !required.is_empty() {
        schema
            .as_object_mut()
            .unwrap()
            .insert("required".into(), Value::Array(required));
    }
    schema
}

fn param_schema(spec: &DeclaredParam) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), Value::String(spec.ty.openapi_type().into()));
    if let Some(f) = spec
        .format
        .clone()
        .or_else(|| spec.ty.openapi_format().map(str::to_string))
    {
        m.insert("format".into(), Value::String(f));
    }
    if let Some(d) = &spec.description {
        m.insert("description".into(), Value::String(d.clone()));
    }
    if let Some(default) = &spec.default {
        m.insert("default".into(), default.clone());
    }
    if let Some(allowed) = &spec.allowed {
        m.insert("enum".into(), Value::Array(allowed.clone()));
    }
    if spec.ty == ParamType::Array {
        let items_type = spec
            .items
            .as_ref()
            .map(|i| i.ty.openapi_type())
            .unwrap_or("string");
        let items_format = spec.items.as_ref().and_then(|i| {
            i.format
                .clone()
                .or_else(|| i.ty.openapi_format().map(str::to_string))
        });
        let mut items = json!({"type": items_type});
        if let Some(f) = items_format {
            items
                .as_object_mut()
                .unwrap()
                .insert("format".into(), Value::String(f));
        }
        m.insert("items".into(), items);
    }
    Value::Object(m)
}

fn build_responses(decl: &Declaration, batch: bool) -> Value {
    let row_schema = returns_schema(decl.returns.as_deref());
    let success_schema = if batch {
        json!({
            "type": "array",
            "items": { "type": "array", "items": row_schema }
        })
    } else {
        json!({
            "type": "array",
            "items": row_schema
        })
    };
    json!({
        "200": {
            "description": "OK",
            "content": {
                "application/json": {"schema": success_schema}
            }
        },
        "400": {
            "description": "Bad Request",
            "content": {
                "application/json": {"schema": {"$ref": "#/components/schemas/Error"}}
            }
        },
        "413": {
            "description": "Payload Too Large",
            "content": {
                "application/json": {"schema": {"$ref": "#/components/schemas/Error"}}
            }
        }
    })
}

fn returns_schema(returns: Option<&[ReturnField]>) -> Value {
    let Some(fields) = returns else {
        return json!({
            "type": "object",
            "additionalProperties": true,
            "description": "Row shape not declared; add `returns:` to the SQL declaration for a typed schema."
        });
    };
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();
    for f in fields {
        let mut prop = Map::new();
        let ty = f.ty.openapi_type().to_string();
        if f.nullable {
            // OpenAPI 3.1 uses type-array to express nullable.
            prop.insert(
                "type".into(),
                Value::Array(vec![Value::String(ty), Value::String("null".into())]),
            );
        } else {
            // If the row column is declared non-nullable, that's a
            // schema-level promise; leave `type` scalar.
            prop.insert("type".into(), Value::String(ty));
        }
        if let Some(fmt) = f
            .format
            .clone()
            .or_else(|| f.ty.openapi_format().map(str::to_string))
        {
            prop.insert("format".into(), Value::String(fmt));
        }
        if let Some(d) = &f.description {
            prop.insert("description".into(), Value::String(d.clone()));
        }
        properties.insert(f.name.clone(), Value::Object(prop));
        if !f.nullable {
            required.push(Value::String(f.name.clone()));
        }
    }
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties
    });
    if !required.is_empty() {
        schema
            .as_object_mut()
            .unwrap()
            .insert("required".into(), Value::Array(required));
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{HttpMethod, QueryIndex};
    use std::path::PathBuf;

    fn build(sql: &str, method: HttpMethod, project: &str, path: &str) -> QueryIndex {
        let (declaration, sql) = crate::declaration::parse(sql).unwrap();
        let mut idx = QueryIndex::default();
        idx.insert(SavedQuery {
            project: project.into(),
            method,
            path: path.into(),
            sql,
            source_file: PathBuf::from(format!("{project}/{}/{path}.sql", method.as_str())),
            transactional: false,
            declaration,
        })
        .unwrap();
        idx
    }

    #[test]
    fn spec_has_openapi_version_and_info() {
        let idx = QueryIndex::default();
        let spec = build_spec(&idx, &OpenApiConfig::default());
        assert_eq!(spec["openapi"], OPENAPI_VERSION);
        assert_eq!(spec["info"]["title"], "Resql");
        assert!(spec["paths"].as_object().unwrap().is_empty());
    }

    #[test]
    fn get_endpoint_emits_query_parameters_with_types() {
        let sql = r#"/*
description: Find users
params:
  login:  { type: string,  required: true }
  active: { type: boolean, required: false }
*/
SELECT :login, :active;
"#;
        let idx = build(sql, HttpMethod::Get, "crm", "users/find");
        let spec = build_spec(&idx, &OpenApiConfig::default());
        let params = &spec["paths"]["/crm/users/find"]["get"]["parameters"];
        let arr = params.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let login = arr.iter().find(|p| p["name"] == "login").unwrap();
        assert_eq!(login["required"], true);
        assert_eq!(login["schema"]["type"], "string");
        let active = arr.iter().find(|p| p["name"] == "active").unwrap();
        assert_eq!(active["required"], false);
        assert_eq!(active["schema"]["type"], "boolean");
    }

    #[test]
    fn post_endpoint_emits_typed_request_body_and_required() {
        let sql = r#"/*
params:
  email:    { type: string,  required: true }
  sendMail: { type: boolean, required: false }
*/
INSERT INTO users (email) VALUES (:email) RETURNING id;
"#;
        let idx = build(sql, HttpMethod::Post, "crm", "users/create");
        let spec = build_spec(&idx, &OpenApiConfig::default());
        let body = &spec["paths"]["/crm/users/create"]["post"]["requestBody"]["content"]
            ["application/json"]["schema"];
        assert_eq!(body["type"], "object");
        assert_eq!(body["properties"]["email"]["type"], "string");
        assert_eq!(body["required"], json!(["email"]));
    }

    #[test]
    fn post_endpoint_gets_batch_variant() {
        let sql = r#"/*
params:
  x: { type: integer, required: true }
*/
INSERT INTO t VALUES (:x);
"#;
        let idx = build(sql, HttpMethod::Post, "audit", "write");
        let spec = build_spec(&idx, &OpenApiConfig::default());
        let batch = &spec["paths"]["/audit/write/batch"]["post"];
        let queries_schema =
            &batch["requestBody"]["content"]["application/json"]["schema"]["properties"]["queries"];
        assert_eq!(queries_schema["type"], "array");
        assert_eq!(
            queries_schema["items"]["properties"]["x"]["type"],
            "integer"
        );
    }

    #[test]
    fn returns_becomes_typed_response_schema() {
        let sql = r#"/*
params: {}
returns:
  - { name: id, type: integer, nullable: false }
  - { name: email, type: string }
*/
SELECT 1 AS id, 'x' AS email;
"#;
        let idx = build(sql, HttpMethod::Get, "crm", "list");
        let spec = build_spec(&idx, &OpenApiConfig::default());
        let items = &spec["paths"]["/crm/list"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["items"];
        assert_eq!(items["properties"]["id"]["type"], "integer");
        assert_eq!(items["required"], json!(["id"]));
        // Nullable defaults on — email becomes a type array.
        assert_eq!(
            items["properties"]["email"]["type"],
            json!(["string", "null"])
        );
    }

    #[test]
    fn enum_on_param_is_emitted_in_schema() {
        let sql = r#"/*
params:
  status: { type: string, required: true, enum: [active, disabled] }
*/
SELECT :status;
"#;
        let idx = build(sql, HttpMethod::Get, "crm", "users/by-status");
        let spec = build_spec(&idx, &OpenApiConfig::default());
        let params = &spec["paths"]["/crm/users/by-status"]["get"]["parameters"];
        let status = params
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "status")
            .unwrap();
        assert_eq!(status["schema"]["type"], "string");
        assert_eq!(status["schema"]["enum"], json!(["active", "disabled"]));
    }

    #[test]
    fn enum_on_post_param_is_emitted_in_request_body_schema() {
        let sql = r#"/*
params:
  status: { type: string, required: true, enum: [active, disabled] }
*/
UPDATE users SET status = :status;
"#;
        let idx = build(sql, HttpMethod::Post, "crm", "users/set-status");
        let spec = build_spec(&idx, &OpenApiConfig::default());
        let props = &spec["paths"]["/crm/users/set-status"]["post"]["requestBody"]["content"]
            ["application/json"]["schema"]["properties"];
        assert_eq!(props["status"]["enum"], json!(["active", "disabled"]));
    }

    #[test]
    fn deterministic_ordering() {
        // Two endpoints inserted in reverse order should still show
        // alpha-sorted paths in the spec.
        let sql_a = "/*\nparams: {}\n*/\nSELECT 1;\n";
        let sql_b = "/*\nparams: {}\n*/\nSELECT 2;\n";
        let mut idx = QueryIndex::default();
        let (decl_b, sql_b_body) = crate::declaration::parse(sql_b).unwrap();
        idx.insert(SavedQuery {
            project: "crm".into(),
            method: HttpMethod::Get,
            path: "b".into(),
            sql: sql_b_body,
            source_file: PathBuf::from("crm/GET/b.sql"),
            transactional: false,
            declaration: decl_b,
        })
        .unwrap();
        let (decl_a, sql_a_body) = crate::declaration::parse(sql_a).unwrap();
        idx.insert(SavedQuery {
            project: "crm".into(),
            method: HttpMethod::Get,
            path: "a".into(),
            sql: sql_a_body,
            source_file: PathBuf::from("crm/GET/a.sql"),
            transactional: false,
            declaration: decl_a,
        })
        .unwrap();
        let spec = build_spec(&idx, &OpenApiConfig::default());
        let paths: Vec<&String> = spec["paths"].as_object().unwrap().keys().collect();
        assert_eq!(paths, vec!["/crm/a", "/crm/b"]);
    }
}
