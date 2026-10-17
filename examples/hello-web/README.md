Run `./examples/hello-web/run` with `hirsel` on PATH, then
`curl http://127.0.0.1:8080/`. Add `--native` for compiled execution.

The script runs `hirsel run main.hir --grants grants.json`, binding the template
to the current program in one step.
It grants only stdout, empty Args, and listen on exactly `127.0.0.1:8080`.
The server handles one connection at a time, reads at most 16 KiB of request head
and 1 MiB of Content-Length body, sends 400 for invalid requests, and closes each
connection after its response. Transfer encodings and persistent connections are
unsupported. Stop it with Ctrl-C.

Error propagation uses the one-argument sum constructors directly (`? Grant`,
`? Network`, `? Http`), as specified by LANG-8. No mapper functions are needed.
