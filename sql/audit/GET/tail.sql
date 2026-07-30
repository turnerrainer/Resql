-- GET /audit/tail?n=<value>
-- Routes to the `audit` datasource by URL segment. Different backend from
-- /users/*. Demonstrates multi-database routing on the demo image.
--   → 200 [{"entry": "...", "row": N}]
SELECT 'audit entry ' || :n AS entry, cast(:n AS INTEGER) AS row_id;
