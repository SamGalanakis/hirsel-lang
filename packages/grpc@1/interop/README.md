# gRPC interop

`main.hir` ports the gRPC interop test server and client
(grpc/grpc `doc/interop-test-descriptions.md`, grpc-go `interop/test_utils.go`) to
grpc@1. `testing.hir` is generated from `proto/` by:

```
protoc@1 --grpc testing.hir test.proto empty.proto messages.proto
```

```
interop server --port=N
interop client --server_host=H --server_port=N --test_case=NAME
```

The client implements `empty_unary`, `large_unary`, `client_streaming`,
`server_streaming`, `ping_pong`, `cancel_after_begin`, `timeout_on_sleeping_server`,
`status_code_and_message`, `custom_metadata`, `empty_stream`,
`cancel_after_first_response`, `special_status_message`, `unimplemented_method`
and `unimplemented_service`. The server implements TestService and deliberately
returns status 12 (UNIMPLEMENTED) for `UnimplementedCall`, matching grpc-go's
unimplemented handler.

Grants need a `listen` rule (server) or a `connect` rule (client) for 127.0.0.1 and
the port, plus the tasks and monotonic clock providers.

## Results against grpc-go

Measured with grpc-go's interop client and server (`interop/client`, `interop/server`,
grpc-go `679b224109d3373eb0e4d80b6e41aafc8d295b6d`), built outside the tree, in both
directions. See the table below.

| Case | Hirsel client, grpc-go server | grpc-go client, Hirsel server |
|---|---|---|
| empty_unary | pass | pass |
| large_unary | pass | pass |
| client_streaming | pass | pass |
| server_streaming | pass | pass |
| ping_pong | pass | pass |
| cancel_after_begin | pass | pass |
| timeout_on_sleeping_server | pass | pass |
| status_code_and_message | pass | pass |
| custom_metadata | pass | pass |
| empty_stream | pass | pass |
| cancel_after_first_response | pass | pass |
| special_status_message | pass | pass |
| unimplemented_method | pass | pass |
| unimplemented_service | pass | pass |

28 of 28 pass, with the Hirsel side built natively (LLVM) on 2026-10-20.

Reproduce with the pinned native toolchain inputs from
[building.md](../../../docs/building.md), the Hirsel CLI, and grpc-go's client
and server built at the commit above:

```sh
hirsel build packages/grpc@1/interop/hirsel.json -o /tmp/grpc-interop-hirsel
python3 packages/grpc@1/interop/check.py --hirsel /tmp/grpc-interop-hirsel \
  --go-client /path/to/grpc-go-client --go-server /path/to/grpc-go-server
```

The runner creates loopback grants and an available port, bounds each case
at 30 seconds, retains per-case logs, and terminates both servers. Source
line citations and every API adaptation are recorded in
[the parity notes](../../../docs/prior-art-web-small.md).
