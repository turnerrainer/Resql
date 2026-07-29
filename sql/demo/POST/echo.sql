-- POST /demo/echo   body: {"msg": "..."}
--   → 200 [{ "echoed": "..." }]
SELECT :msg AS echoed;
