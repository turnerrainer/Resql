# Postman assets

Two files ready to import into Postman:

- `resql.postman_collection.json` — six requests covering `/health`,
  `/datasources`, `/openapi.json`, the four demo endpoints (`/users/hello`,
  `/users/echo`, `/audit/tail`, `/audit/write`), the auto-emitted
  `/…/batch` variants, and a "**Declaration behaviour**" folder that
  demonstrates the three 400 shapes from task 008 (missing required,
  unknown key, wrong type).
- `resql.postman_environment.json` — one variable, `baseUrl`,
  defaulting to `http://localhost:8080`.

## Import

Postman → File → Import → drop both files. Select the "Resql — local"
environment in the top-right dropdown.

## Regenerate from the live spec

Resql serves the full OpenAPI 3.1 document at `/openapi.json`. If you
add endpoints (new `.sql` files with declarations), regenerate the
collection so it stays in sync:

```bash
# Start Resql locally with your project's sql/ tree wired in.
docker run --rm -p 8080:8080 turnerrainer/resql:0.1.0-alpha.3

# Snapshot the live spec.
curl -s http://localhost:8080/openapi.json \
    > postman/openapi.json

# Convert. Requires Node.js.
npx openapi-to-postmanv2 \
    -s postman/openapi.json \
    -o postman/resql.postman_collection.json \
    -p
```

Re-import the regenerated collection into Postman (File → Import →
select and replace).

## What the "Declaration behaviour" folder demonstrates

Run each request against the shipped demo image and observe the
response body:

| Request | Status | `error` |
|---|---|---|
| `POST /users/echo` with `{}` | 400 | `InvalidDataAccessApiUsageException` |
| `POST /users/echo` with `{"msg":"pong","extra":"not declared"}` | 400 | `UnknownParameterException` |
| `GET /audit/tail?n=not-an-int` | 400 | `InvalidParameterTypeException` |

These are the three new validation error shapes introduced by
task 008 (mandatory declaration + typed request validation). See
[Declarations & OpenAPI](../book/src/declarations.md) for the full
model.
