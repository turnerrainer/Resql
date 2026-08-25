/*
description: Record a synthetic audit entry (demo endpoint).
namespace: audit
params:
  actor:
    type: string
    required: true
  action:
    type: string
    required: true
returns:
  - name: actor
    type: string
    nullable: false
  - name: action
    type: string
    nullable: false
  - name: servedFrom
    type: string
    nullable: false
*/
-- POST /audit/write  body: {"actor": "...", "action": "..."}
-- Routes to the `audit` datasource. Different backend from /users/*.
--   → 200 [{"actor": "...", "action": "...", "servedFrom": "audit"}]
SELECT :actor AS actor, :action AS action, 'audit' AS served_from;
