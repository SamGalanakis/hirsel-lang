Run `./examples/hello-web-concurrent/run` with `hirsel` on PATH, then
`curl http://127.0.0.1:8080/`. Structured concurrency currently runs in the interpreter.

The script exports the current semantic program binding and applies `grants.json`.
It grants stdout, cooperative tasks, empty Args, and listen on exactly `127.0.0.1:8080`.
The server serves each connection in a scoped child task, reads at most 16 KiB of request head
and 1 MiB of Content-Length body, sends 400 for invalid requests, and closes each
connection after its response. Transfer encodings and persistent connections are
unsupported. Stop it with Ctrl-C.
