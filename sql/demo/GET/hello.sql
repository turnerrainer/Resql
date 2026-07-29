-- GET /demo/hello?name=<value>
--   → 200 [{ "greeting": "hello, <value>!" }]
SELECT 'hello, ' || :name || '!' AS greeting;
