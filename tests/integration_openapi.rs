//! End-to-end tests for the `/openapi.json` route (task 008).

mod common;
use common::TestAppBuilder;

#[tokio::test]
async fn openapi_route_returns_valid_document() {
    let app = TestAppBuilder::new()
        .with_sql(
            "crm/GET/users/find.sql",
            "/*\ndescription: Look up users by login.\nparams:\n  login:  { type: string, required: true }\n  status: { type: string, required: false }\nreturns:\n  - { name: id,    type: integer, nullable: false }\n  - { name: email, type: string }\n*/\nSELECT :login, :status;",
        )
        .with_sql(
            "crm/POST/users/create.sql",
            "/*\nparams:\n  email: { type: string, required: true }\n*/\nSELECT :email;",
        )
        .build()
        .await;

    let (status, spec) = app.request("GET", "/openapi.json", None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(spec["openapi"], "3.1.0");
    assert!(spec["info"]["title"].is_string());

    // GET endpoint's query parameters typed and required-flagged.
    let get_params = &spec["paths"]["/crm/users/find"]["get"]["parameters"];
    let arr = get_params.as_array().unwrap();
    let login = arr.iter().find(|p| p["name"] == "login").unwrap();
    assert_eq!(login["required"], true);
    assert_eq!(login["schema"]["type"], "string");
    let status_p = arr.iter().find(|p| p["name"] == "status").unwrap();
    assert_eq!(status_p["required"], false);

    // Returns → 200 response schema.
    let ok_items = &spec["paths"]["/crm/users/find"]["get"]["responses"]["200"]["content"]
        ["application/json"]["schema"]["items"];
    assert_eq!(ok_items["properties"]["id"]["type"], "integer");
    // email is nullable by default → type is array [string, null].
    assert_eq!(
        ok_items["properties"]["email"]["type"],
        serde_json::json!(["string", "null"])
    );

    // POST endpoint gets a request body AND an auto-generated batch
    // variant at /crm/users/create/batch.
    assert!(spec["paths"]["/crm/users/create"]["post"]["requestBody"].is_object());
    assert!(spec["paths"]["/crm/users/create/batch"]["post"].is_object());
}

#[tokio::test]
async fn missing_optional_param_still_bound_via_e2e() {
    // Cross-check the runtime behaviour that the /openapi.json spec
    // advertises: an optional param can be omitted from the request and
    // the SQL sees NULL, not a MissingParameter error.
    let app = TestAppBuilder::new()
        .with_seed(
            "CREATE TABLE u (name TEXT, tier TEXT); \
             INSERT INTO u VALUES ('alice', 'gold'); \
             INSERT INTO u VALUES ('bob',   NULL);",
        )
        .with_sql(
            "demo/GET/list.sql",
            "/*\nparams:\n  tier: { type: string, required: false }\n*/\nSELECT name FROM u WHERE (:tier IS NULL OR tier = :tier) ORDER BY name",
        )
        .build()
        .await;
    let (status, body) = app.request("GET", "/demo/list", None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        serde_json::json!([{"name": "alice"}, {"name": "bob"}])
    );
}
