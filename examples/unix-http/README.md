# Unix-socket HTTP

The program listens on a Unix domain socket, sends one HTTP request to it through
the std HTTP client with `unix_socket` set (as `curl --unix-socket` does), accepts
that connection, answers it with the std HTTP server and prints the exchange:

```sh
hirsel run examples/unix-http/hirsel.json --grants examples/unix-http/grants.json
hirsel run --native examples/unix-http/hirsel.json --grants examples/unix-http/grants.json
```

Both print `request /status; status 200; body served /status`.

The socket path is the first argument, and the grants name it exactly: a
`unix_listen_remove_stale` rule, so a socket file left by an earlier run that died is
replaced, and a `unix_connect` rule. Closing the listener removes the socket file. Change
the path in both places to move it; no TCP rule is needed. The listener's backlog holds
the client's connection until the program accepts it, so the exchange runs without tasks
and records and replays like any network run (`--record`, `--replay`).
