/*
description: Return an audit-log entry stub keyed by row number.
namespace: audit
params:
  n:
    type: integer
    required: true
    description: Row number to fetch
returns:
  - name: entry
    type: string
    nullable: false
  - name: rowId
    type: integer
    nullable: false
*/
-- GET /audit/tail?n=<value>
-- Routes to the `audit` datasource by URL segment. Different backend from
-- /users/*. Demonstrates multi-database routing on the demo image.
--   → 200 [{"entry": "...", "rowId": N}]
SELECT 'audit entry ' || :n AS entry, cast(:n AS INTEGER) AS row_id;
