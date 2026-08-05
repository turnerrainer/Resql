-- Shared schema for cross-implementation compat runs.
-- Both Java Resql and Rust Resql point at a Postgres 16 loaded with this.
--
-- Contents mirror Java's src/test/resources/init-crm-db.sql minimally so
-- the ported SQL queries in compat/templates/ return the same rows.

BEGIN;

CREATE TABLE IF NOT EXISTS "user" (
    id BIGSERIAL PRIMARY KEY,
    login VARCHAR(255) NOT NULL UNIQUE,
    email VARCHAR(255) NOT NULL,
    name VARCHAR(255) NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

INSERT INTO "user" (id, login, email, name, password_hash, created_at) VALUES
    (-1, 'admin', 'admin@example.com', 'Admin Name',
        '$2a$12$YlbrvfwwznrmQNM71UFFvO3krrFnUsKvGcN5zNDBNMpD2w9WDqHuO',
        '2021-11-26T14:52:32.748+00:00'),
    (-2, 'user',  'user@example.com',  'User Name',
        '$2a$12$AXElLQmIKy1EZVSrlO2HnO0dTsHcf4LstadG7a5arYXUAGf5VCeZm',
        '2021-11-27T15:30:00.000+00:00')
ON CONFLICT (login) DO NOTHING;

COMMIT;
