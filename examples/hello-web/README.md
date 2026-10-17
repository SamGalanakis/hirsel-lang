Run `./examples/hello-web/run` with `hirsel` on PATH, then
`curl http://127.0.0.1:8080/`. Add `--native` for compiled execution.

The script exports the current semantic program binding and applies `grants.json`.
It grants only stdout, empty Args, and listen on exactly `127.0.0.1:8080`.
The server handles one connection at a time, reads at most 16 KiB of request head
and 1 MiB of Content-Length body, sends 400 for invalid requests, and closes each
connection after its response. Transfer encodings and persistent connections are
unsupported. Stop it with Ctrl-C.
