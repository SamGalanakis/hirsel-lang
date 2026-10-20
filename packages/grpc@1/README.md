# grpc@1

gRPC for Hirsel, over the std HTTP/2 implementation (`std@1/http2`) on plaintext
TCP (h2c with prior knowledge). It provides a client and a server, with unary,
client-streaming, server-streaming and bidirectional calls. It also covers
metadata, deadlines, status codes, messages and trailers, and the gRPC
length-prefixed message framing with optional gzip. It is a package, not part of std:

```json
{ "manifest": "<path>/packages/grpc@1/hirsel.json", "package": { "name": "grpc", "version": "1" }, "expose": [] }
```

Service stubs come from `protoc@1 --grpc` (see `packages/protobuf@1/README.md` and
`protoc/gen_grpc.hir` for the stub shapes). `examples/grpc-greeter` is a complete
program. `interop/` runs the gRPC interop cases against grpc-go
(`interop/README.md`).

## Client

- `dial(net, host, port, tasks)` returns a `ClientConn`. It connects on its first
  call and reconnects after a failed or abandoned connection.
- `start(mut conn, path, options)` opens a call.
- `send_request` sends one request message and `close_send` half-closes the call.
- `receive_response` returns the next response, `None` once the call ended OK, or
  the non-OK `Status`.
- `receive_one` returns the only response of a call that has exactly one.
- `finish_call` drains the remaining responses and returns the status.
- `cancel` resets the call's stream with CANCEL.
- `response_headers` and `response_trailers` read the server's metadata.
- `unary(mut conn, path, request, options)` makes a whole unary call and returns a
  `Reply` with the message and both metadata sets.

Calls on one `ClientConn` can interleave: each stream's frames are queued until that
call reads them.

`CallOptions` carry metadata, an optional `Deadline`, gzip compression of requests,
and the largest response accepted (4 MiB by default). The deadline is sent as
`grpc-timeout`. When the connection was dialed with `Tasks`, each blocking read is
bounded with `task.scope_until`. A deadline that passes between reads cancels the
stream. One that expires during a read abandons the connection, since the
interrupted read leaves its frame state unknown, and the next call reconnects.
Either way the call ends `DEADLINE_EXCEEDED`.

## Server

- `serve(net, take stream, options, handler)` serves one accepted connection until
  the client closes it. It runs `handler` for each call. The handler's `Error`
  becomes the call's status, and `Ok` sends `OK`. Accept connections yourself and
  run one `serve` per connection, usually each in its own task.
- In a handler, `method` gives the request path and `request_metadata` the request
  metadata. `receive_request` and `receive_unary` read requests, and
  `send_response` sends a response.
- `set_header` and `set_trailer` add response metadata. Headers go with the first
  response, trailers with the status.
- `deadline` and `timeout` expose the client's `grpc-timeout`.
- `canceled` reports a client reset.

`ServerOptions` carry the largest request accepted, whether responses are
gzip-compressed for clients that accept it, and the clock that turns
`grpc-timeout` into `deadline`.

Calls on one connection are served one at a time, in arrival order. Frames for
other calls wait in their stream's queue. A client that needs two calls on one
connection to progress together, such as one waiting for a response the other's
handler would produce, should use two connections.

## Wire details

- Requests carry `content-type: application/grpc`, `te: trailers` and
  `grpc-accept-encoding: gzip`.
- A response that fails before any message is sent is trailers-only.
- `grpc-message` is percent-encoded as grpc-go encodes it.
- Metadata keys are lowercase. Keys ending in `-bin` carry bytes, sent as
  unpadded base64; padded values are also accepted.
- The reserved headers (`grpc-*`, `content-type`, `te`, `user-agent` and pseudo
  headers) cannot be sent as metadata.
- Non-200 HTTP responses map to codes as grpc-go maps them.
