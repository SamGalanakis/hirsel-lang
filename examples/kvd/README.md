TCP 127.0.0.1:18765: `SET key value`, `GET key`, `DEL key`; commits become durable before replies. SIGTERM cancels the listener/session and closes std store; a Mutex counts served commands.
`hirsel test examples/kvd/hirsel.json`
`hirsel run examples/kvd/hirsel.json --grants examples/kvd/session-grants.json --record /tmp/kvd.tape`
`hirsel run --native examples/kvd/hirsel.json --grants examples/kvd/session-grants.json`
`hirsel run examples/kvd/hirsel.json --replay /tmp/kvd.tape` (also accepts `--native`). Use `grants.json` for a live listener.
