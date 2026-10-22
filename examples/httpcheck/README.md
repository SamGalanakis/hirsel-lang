# httpcheck

Run a bounded sequence through the shipped HTTP client:

```sh
hirsel run examples/httpcheck/hirsel.json --grants /tmp/httpcheck-grants.json -- \
  '{"steps":[{"method":"GET","url":"http://127.0.0.1:8080/items","headers":[],"expect":{"status":200,"contains":"wool","json":[{"pointer":"/items/0/name","equals":"wool"}]}}]}'
```

Pass exactly one JSON argument. All objects reject duplicate, missing required
and unknown keys. The entire scenario is validated before traffic. There is no
environment interpolation, retry, parallel request, regex or JSON Patch dependency.
Request/response contents, URLs, headers and expected values never appear in the
report or error messages. Tapes contain inputs and effects; keep recordings and
scenario arguments private when they contain secrets.

The root has only `steps`, an array of 1–16 steps. Each step requires `method`,
`url`, `headers` and `expect`; `body` is an optional UTF-8 string. Methods are
exactly GET, HEAD, POST, PUT, DELETE, PATCH and OPTIONS. HEAD rejects a body.
URLs must be absolute HTTP(S), at most 4096 UTF-8 bytes, with a valid host and
request target. User information, fragments and port zero are rejected. The
shipped URI parser and HTTP host/target conversion validate without dialing.
Endpoint authority is checked by the client on each connection.

Headers are ordered arrays of `{"name":"…","value":"…"}` records, preserving
duplicates. Arrays have at most 32 records, names at most 256 bytes and values
at most 4096 bytes, with HTTP token/control-byte validation. Request Host,
Content-Length, Transfer-Encoding, Connection, Expect and Trailer are reserved
for the client and rejected. A header predicate matches any response field with
an ASCII case-insensitive name and an exact value; fields are never joined and
later duplicate fields (including Set-Cookie) remain available.

`expect` requires integer `status` (100–599) and optionally accepts:

- `headers`: header predicates in the same array format.
- `contains`: a literal UTF-8 substring, including an empty string.
- `json`: up to 32 `{"pointer":"/path","equals":<JSON value>}` predicates.
  The shipped RFC 6901 parser handles `~0`, `~1` and the empty root pointer;
  invalid escapes fail scenario validation. Missing paths differ from JSON null.
  Equality uses existing `json.Value` structural equality, including Hirsel
  Map insertion order: object member order matters, including at nested paths.
  Number lexemes are exact (`1` differs from `1.0`).

The scenario is at most 65536 UTF-8 bytes; a request body is at most 16384.
Each request budgets at most 32768 bytes: encoded request target, Host value,
scenario headers with separators, body and a 2048-byte client framing allowance. Each response caps headers at 16384 bytes and
64 fields and raw body at 65536 bytes, including chunked/EOF-delimited bodies.
Compression is not requested or decoded. Text predicates require valid UTF-8;
JSON predicates require strict JSON; malformed response data is `transport_error`.
Redirects are disabled (`max_redirects=0`): the shipped client reports an error
rather than following them. Each exchange has a two-second whole-request
deadline with `task.scope_until`, plus two-second connect/read/write timeouts.
Sixteen exchanges have at most 32 seconds of deadlines, excluding CPU validation,
reporting and host scheduling. Request resources close on return.

The program buffers a deterministic JSON line for every step, then a summary:

```json
{"step":0,"result":"assertion_failure","failures":["status","header","contains","json"]}
{"step":1,"result":"pass","failures":[]}
{"summary":{"passed":1,"failed":1,"errors":0,"skipped":0}}
```

Indices are zero-based. Failed predicates appear in fixed order: status, headers
in scenario order, contains, JSON pointers in scenario order. Repeated failures
retain repeated names. Assertion mismatches allow later steps to run. A
transport/protocol/deadline error emits `transport_error` and marks remaining
steps `skipped`. Invalid scenarios emit `{"result":"scenario_error"}` and a
summary with one error and no step rows. Missing network/task/clock authority and denied endpoints produce transport
errors without traffic. Missing stdout returns `Authority`; output device
failure returns `Output`.

Exit status is **0 for all pass, 1 for any application error or mismatch**.
The shipped language requires `main -> Result[(), E]`: every `Error(e)` exits 1,
independently of its variant. The JSON report distinguishes scenario, transport
and assertion failures. This example preserves that policy without an exit-code
API or launcher workaround. The host writes `error: Scenario`, `Transport`,
`Assertion`, `Authority` or `Output` plus LF on stderr for a returned error.

For the invocation above, `/tmp/httpcheck-grants.json` explicitly grants just
the endpoint, stdout, cooperative tasks and a monotonic clock:

```json
{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[],"net":{"provider":"net.stream@1","rules":[{"address":"127.0.0.1","service":"8080","operation":"connect"}]}},"args":[]}
```

Build each native backend once and reuse its executable with the matching
[native toolchain and std image](../../docs/native-verification.md):

```sh
hirsel build examples/httpcheck/hirsel.json --backend llvm -o /tmp/httpcheck-llvm
hirsel build examples/httpcheck/hirsel.json --backend cranelift -o /tmp/httpcheck-cranelift
scenario='{"steps":[{"method":"GET","url":"http://127.0.0.1:8080/items","headers":[],"expect":{"status":200}}]}'
hirsel run examples/httpcheck/hirsel.json --grants /tmp/httpcheck-grants.json \
  --record /tmp/httpcheck-h.tape -- "$scenario"
/tmp/httpcheck-llvm --hirsel-record /tmp/httpcheck-llvm.tape \
  --hirsel-grants /tmp/httpcheck-grants.json "$scenario"
/tmp/httpcheck-cranelift --hirsel-record /tmp/httpcheck-cranelift.tape \
  --hirsel-grants /tmp/httpcheck-grants.json "$scenario"
# Stop your server. Replay needs no grants, stdin or scenario operands.
hirsel run examples/httpcheck/hirsel.json --replay /tmp/httpcheck-h.tape
/tmp/httpcheck-llvm --hirsel-replay /tmp/httpcheck-h.tape
/tmp/httpcheck-cranelift --hirsel-replay /tmp/httpcheck-h.tape
```

Run the focused integration check from the checkout root:

```sh
python3 tools/buck2/prepare.py
.buck2/bin/buck2 test //:hirsel-cli-test-cli-shard-1 -- --test-arg=httpcheck_15
```

The harness binds port zero, refuses occupied ports, retains its listener and
cleans only its own server. It checks exact requests, ordered methods/body/header
vectors, status/header/body/JSON mismatches, zero traffic on invalid scenarios,
grants, redirects, limits, truncation and a trickled response that must hit the
whole-request deadline. H, LLVM and Cranelift each record success, mismatch and
transport failure; all engines replay all nine tapes after the listener closes,
grants are removed and Linux seccomp denies socket/connect in every replay
child (using libseccomp.so.2). Complete stdout,
stderr and exit status must agree. Commands, failed/setup/timeout cells and real
executed engine counts are retained in external scratch; empty selections fail.

Prior art: Go `net/http/client_test.go:63–148` and
`net/http/httptest/server.go:266–310` at
`ebe2feefc400f813aefdc8a036e66d2619da6649`, Copyright 2009 The Go Authors,
BSD-3-Clause (license in [third-party/NOTICES.md](../../third-party/NOTICES.md)).
The GET `User-agent:` body, HEAD Last-Modified and POST `{"key":"value"}` and
Content-Length vectors are independently adapted. The retained private listener
and owned close lifecycle follow httptest. Semantic adaptations: explicit grants
replace ambient networking; scripted HTTP/1.1 peers replace Go handlers; ordered
scenario predicates replace individual tests; bounded deadlines, full-output
goldens and offline replay enforce Hirsel's contracts. Existing
[httpaggregate](../httpaggregate/main.hir) supplies the explicit HTTP configuration
pattern. No shipped modules change.
