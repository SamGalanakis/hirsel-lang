A JSON todo API. Run `./examples/todo-api/run` with `hirsel` on PATH (`./run 9000` picks
another port; `./run 0` picks a free one and prints it). Stop it with Ctrl-C.

```sh
curl -s localhost:8080/todos
curl -s -X POST -H 'Content-Type: application/json' -d '{"title":"feed the sheep"}' localhost:8080/todos
curl -s localhost:8080/todos/1
curl -s -X PATCH -H 'Content-Type: application/json' -d '{"done":true}' localhost:8080/todos/1
curl -s -X DELETE localhost:8080/todos/1
```

| Request | Success | Errors |
| --- | --- | --- |
| `GET /todos` | 200, array of `{"id","title","done"}` | |
| `POST /todos` `{"title": string}` | 201, the new todo | 400 bad JSON or title, 415 not `application/json` |
| `GET /todos/<id>` | 200, the todo | 404 |
| `PATCH /todos/<id>` `{"done": bool}` | 200, the todo | 400, 404, 415 |
| `DELETE /todos/<id>` | 204 | 404 |

Unknown paths are 404, other methods 405 with `Allow`, and malformed HTTP 400; every error
body is `{"error": message}`. Ids are never reused.

The script grants stdout, cooperative tasks, listen on exactly `127.0.0.1:<port>` and
read-write access to `./data`. The shipped template names that directory relative to
itself; the wrapper also selects the requested listen port. The
state file `data/todos.json` is loaded at startup (missing means empty) and replaced
atomically after every change, before the response is sent. Each connection runs in its
own task; the list lives in a `Mutex`, and one writer task owns the file, skipping a
snapshot when a newer one has already been written. The server reads at most 16 KiB of
head and 64 KiB of body, closes each connection after its response, and runs in the
interpreter only, since native tasks are not available yet. Host runs have no default
step budget, so the server can keep accepting requests.
