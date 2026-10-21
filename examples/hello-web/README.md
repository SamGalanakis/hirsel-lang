Run `./examples/hello-web/run` with `hirsel` on PATH, then
`curl http://127.0.0.1:8080/`. Add `--native` for compiled execution.

The script runs `hirsel run main.hir --grants grants.json`, binding the template
to the current program in one step.
It grants stdout, cooperative Tasks, empty Args, and listen on exactly `127.0.0.1:8080`.
The server handles one connection at a time, reads at most 16 KiB of request head
and 1 MiB of decoded body (Content-Length or chunked), sends 400 for invalid
requests, and closes each connection after its inline response;
[hello-web-concurrent](../hello-web-concurrent/README.md) keeps connections open.
A failed request closes its connection and the accept loop continues.
Stop it with Ctrl-C.

Error propagation wraps each library error in the one arm of `ServerError` that
carries it (LANG-8). A plain `?` selects that arm itself, as in `net.accept(..)?`;
writing the arm, as in `? Network`, means the same. No mapper functions are needed.
