-- POST /audit/write  body: {"actor": "...", "action": "..."}
-- Routes to the `audit` datasource. Different backend from /users/*.
--   → 200 [{"actor": "...", "action": "...", "servedFrom": "audit"}]
SELECT :actor AS actor, :action AS action, 'audit' AS served_from;
