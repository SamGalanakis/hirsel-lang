Run `./examples/hello-web-concurrent/run` with `hirsel` on PATH, then
`curl http://127.0.0.1:8080/`. The server runs interpreted or compiled with either
native backend.

The script exports the current semantic program binding and applies `grants.json`.
It grants stdout, cooperative tasks, a monotonic clock, empty Args, and listen on exactly `127.0.0.1:8080`.
Each connection runs in a scoped child task, with sequential requests and ordered
pipelined responses. HTTP/1.1 stays open until the client asks to close or the
30-second idle limit expires. A separate 30-second whole-request read deadline
covers headers, body and trailers; a slow request gets 408 and closes. HTTP/1.0 requires Connection: keep-alive. The server
reads at most 16 KiB of request head and 1 MiB of decoded Content-Length or chunked body, and sends
400 for invalid requests. Stop it with Ctrl-C.
