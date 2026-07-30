-- POST /users/echo   body: {"msg": "..."}
--   → 200 [{"echoed": "...", "servedFrom": "users"}]
SELECT :msg AS echoed, 'users' AS served_from;
