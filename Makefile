# Resql developer make targets.
#
# The Rust build itself never touches these — they're conveniences for
# the Postgres + Liquibase story documented in book/src/postgres-setup.md.
# CI does the same steps inline; nothing here is required in production.

PG_CONTAINER    ?= resql-test-pg
PG_IMAGE        ?= postgres:16
PG_PASSWORD     ?= cipw
PG_DB           ?= resql_test
PG_HOST_PORT    ?= 5433
LIQUIBASE_IMAGE ?= liquibase/liquibase:4.29

PG_URL          := postgres://postgres:$(PG_PASSWORD)@localhost:$(PG_HOST_PORT)/$(PG_DB)
PG_URL_JDBC     := jdbc:postgresql://localhost:$(PG_HOST_PORT)/$(PG_DB)

.PHONY: help
help:
	@echo "targets:"
	@echo "  test           — SQLite-only integration suite (fast)"
	@echo "  test-pg        — Postgres integration suite (spins pg-up + pg-schema)"
	@echo "  test-all       — SQLite + Postgres suites together"
	@echo "  pg-up          — start local Postgres container on port $(PG_HOST_PORT)"
	@echo "  pg-schema      — apply Liquibase changelog with --contexts=test"
	@echo "  pg-schema-prod — apply Liquibase changelog WITHOUT test fixtures"
	@echo "  pg-down        — stop + remove local Postgres container"
	@echo "  pg-shell       — psql into the running local Postgres"
	@echo "  fmt / lint     — cargo fmt / clippy -D warnings"
	@echo "  audit          — cargo audit --deny warnings"
	@echo "  book           — mdbook build"

.PHONY: fmt lint audit book test test-all
fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets --locked -- -D warnings

audit:
	cargo audit --deny warnings

book:
	mdbook build book

test:
	cargo test --no-fail-fast --locked

test-all: pg-up pg-schema
	TEST_POSTGRES_URL="$(PG_URL)" cargo test --no-fail-fast --locked

# ─── Postgres lifecycle ──────────────────────────────────────────────

.PHONY: pg-up pg-down pg-schema pg-schema-prod pg-shell test-pg

pg-up:
	@if [ -z "$$(docker ps -q -f name=^$(PG_CONTAINER)$$)" ]; then \
	   docker rm -f $(PG_CONTAINER) 2>/dev/null || true; \
	   docker run -d --name $(PG_CONTAINER) \
	     -e POSTGRES_PASSWORD=$(PG_PASSWORD) \
	     -e POSTGRES_DB=$(PG_DB) \
	     -p $(PG_HOST_PORT):5432 \
	     $(PG_IMAGE) >/dev/null; \
	   printf 'waiting for Postgres on :%s ' "$(PG_HOST_PORT)"; \
	   for _ in $$(seq 1 30); do \
	     if docker exec $(PG_CONTAINER) pg_isready -U postgres >/dev/null 2>&1; then \
	       echo " up"; exit 0; \
	     fi; \
	     printf .; sleep 1; \
	   done; \
	   echo " TIMEOUT"; exit 1; \
	 else \
	   echo "$(PG_CONTAINER) already running on :$(PG_HOST_PORT)"; \
	 fi

pg-down:
	@docker rm -f $(PG_CONTAINER) 2>/dev/null || true
	@echo "$(PG_CONTAINER) stopped."

pg-schema:
	docker run --rm --network host \
	  -v "$(CURDIR)/db/changelog:/liquibase/changelog" \
	  $(LIQUIBASE_IMAGE) \
	  --url=$(PG_URL_JDBC) \
	  --username=postgres --password=$(PG_PASSWORD) \
	  --changeLogFile=changelog/master.yaml \
	  --contexts=test \
	  update

pg-schema-prod:
	docker run --rm --network host \
	  -v "$(CURDIR)/db/changelog:/liquibase/changelog" \
	  $(LIQUIBASE_IMAGE) \
	  --url=$(PG_URL_JDBC) \
	  --username=postgres --password=$(PG_PASSWORD) \
	  --changeLogFile=changelog/master.yaml \
	  update

pg-shell:
	docker exec -it $(PG_CONTAINER) psql -U postgres -d $(PG_DB)

test-pg: pg-up pg-schema
	TEST_POSTGRES_URL="$(PG_URL)" cargo test --no-fail-fast --locked --test integration_postgres
