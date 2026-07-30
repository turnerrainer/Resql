# Postgres + Liquibase setup

Resql-on-Rust does **not** manage schema. Schema and migrations are
Liquibase's job; Resql just executes SQL that assumes the schema is
already there. This chapter shows the pattern for local test runs and
for production deployments.

## Why this split

- Resql's binary stays small (no JVM, no Liquibase runtime linked in).
- Schema changes go through the same review + rollback story as every
  other Liquibase-managed project in your infrastructure.
- Test fixtures reuse the same changelog machinery — one source of
  truth for "what rows should exist for the integration suite."

## Directory layout

```
db/changelog/
├── master.yaml               # includes the ordered changesets below
├── 001-schema.yaml           # tables, indexes, constraints — always applied
└── 002-test-fixtures.yaml    # seed data, context: test — skipped in prod
```

Every changeset in `002-test-fixtures.yaml` carries `context: test`. That
gate is enforced by Liquibase, not by Resql — passing `--contexts=test`
applies fixtures; omitting the flag applies schema only.

## Local test workflow

The `Makefile` at the repo root wraps the three steps:

```bash
make pg-up        # start Postgres 16 on :5433 in a throwaway container
make pg-schema    # apply master.yaml with --contexts=test
make test-pg     # run cargo test with TEST_POSTGRES_URL set
```

Or the combined shortcut:

```bash
make test-all     # pg-up → pg-schema → cargo test (SQLite + Postgres suites)
```

Without a running Postgres and `TEST_POSTGRES_URL`, the Postgres tests
skip silently and the SQLite suite still runs to completion. Every
integration test is designed to skip cleanly, so `cargo test` never
fails just because Postgres isn't up.

Teardown:

```bash
make pg-down      # remove the throwaway container
```

Container port defaults to 5433 (not 5432) so a local dev Postgres you
might already be running doesn't collide.

## CI workflow

`.github/workflows/tests.yml` runs on both `ubuntu-latest` and
`ubuntu-24.04-arm`. Each job:

1. Starts a `postgres:16` service container on `:5432`.
2. Runs `liquibase update --contexts=test` against it via
   `docker run --network host liquibase/liquibase:4.29`.
3. Exports `TEST_POSTGRES_URL` so the Rust suite picks it up.
4. Runs `cargo test --no-fail-fast --locked`.

No secrets are involved; the CI Postgres password is a fixed
throwaway value that only lives for the length of one job.

## Production deployment

The runtime image ships without Liquibase. Apply schema out-of-band
before (or beside) your Resql pods. Two common patterns:

### Kubernetes init container

```yaml
initContainers:
  - name: db-migrate
    image: liquibase/liquibase:4.29
    args:
      - --url=jdbc:postgresql://pg.internal:5432/appdb
      - --username=migrator
      - --password=$(PGPASSWORD)
      - --changeLogFile=changelog/master.yaml
      - update
    env:
      - name: PGPASSWORD
        valueFrom:
          secretKeyRef:
            name: pg-secrets
            key: migrator-password
    volumeMounts:
      - name: changelog
        mountPath: /liquibase/changelog
```

Note the absence of `--contexts=test`. Production applies schema only.

### One-shot job

`kubectl create job schema-2026-08-01 --image=liquibase/liquibase:4.29 -- ...`
before rolling out the new Resql version. Idempotent — Liquibase records
applied changesets in `databasechangelog`, so re-running is a no-op.

### Kubernetes Job with wait-for

Wrap the init or job with a readiness probe that gates the Resql
deployment on `SELECT 1 FROM databasechangelog WHERE id = '<expected-id>'`.
Resql itself will start regardless of schema — a missing table only
shows up on the first request as `BadSqlGrammarException` (HTTP 400).

## FAQ

**Can I use Flyway instead?** — Yes. Nothing in Resql cares. The `db/`
directory shape is just a convention; swap in `db/flyway/` and
`flyway migrate` if that's your team's tool.

**Can I skip Liquibase and write raw SQL migrations?** — Yes. The
architectural rule is only "Resql does not touch schema" — how you get
schema in is your choice. The Rust integration tests assume the schema
in `db/changelog/001-schema.yaml`; if you want to use a different tool,
either recreate that schema by hand or keep the Liquibase files around
for tests and use your preferred tool in prod.

**Why is `002-test-fixtures.yaml` in the repo?** — Because
`tests/integration_postgres.rs` reads the rows it defines. Keeping
schema + fixtures together as one Liquibase changelog is the point of
the pattern — deleting the fixtures file breaks the test suite in a
useful way (missing rows), not a mysterious way (schema drift).
