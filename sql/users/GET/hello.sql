/*
description: Greet the caller from the `users` datasource.
namespace: users
params:
  name:
    type: string
    required: true
    description: Who to greet
returns:
  - name: greeting
    type: string
    nullable: false
*/
-- GET /users/hello?name=<value>
-- Routes to the `users` datasource by URL segment.
--   → 200 [{"greeting": "hello from users db, <value>!"}]
SELECT 'hello from users db, ' || :name || '!' AS greeting;
