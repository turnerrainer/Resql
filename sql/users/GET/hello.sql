-- GET /users/hello?name=<value>
-- Routes to the `users` datasource by URL segment.
--   → 200 [{"greeting": "hello from users db, <value>!"}]
SELECT 'hello from users db, ' || :name || '!' AS greeting;
