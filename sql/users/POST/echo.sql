/*
description: Echo the caller-supplied message back with a datasource tag.
namespace: users
params:
  msg:
    type: string
    required: true
    description: Message to echo
returns:
  - name: echoed
    type: string
    nullable: false
  - name: servedFrom
    type: string
    nullable: false
*/
-- POST /users/echo   body: {"msg": "..."}
--   → 200 [{"echoed": "...", "servedFrom": "users"}]
SELECT :msg AS echoed, 'users' AS served_from;
