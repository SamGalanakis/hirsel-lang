# localrpc

A small local helper for an agent that needs a bounded request/reply exchange
without an HTTP stack. `serve SOCKET` accepts one client and handles up to eight
requests sequentially. `call SOCKET REQUEST` sends one JSON argument, checks its
reply against the request ID and expected response type, and prints canonical JSON.
There are no tasks, background workers or filesystem input reads.

For example, an agent can normalize a draft label before storing it:

```json
{"id":"draft-42","method":"normalize","text":"  PLEASE Review: Straße 羊\n"}
```

The reply is `{"id":"draft-42","result":"please review: Straße 羊"}`.
[fixtures/normalize.json](fixtures/normalize.json) contains this input;
[fixtures/invalid.json](fixtures/invalid.json) demonstrates an agent accidentally
passing a token list instead of a string. That input produces `{"error":"shape"}`
and exit 2, before connecting.

From the repository root, `hirsel run examples/localrpc/hirsel.json --grants
examples/localrpc/grants.json` prints help and exits 0. To run a real exchange,
use a private directory and grants with the exact absolute socket path:

```sh
work=$(mktemp -d /tmp/localrpc.XXXXXX)
printf '%s\n' "$work"
python3 - "$work" <<'PY'
import json, sys
from pathlib import Path
root = Path(sys.argv[1])
socket = str(root / 'rpc.sock')
for mode, operation in [('serve', 'unix_listen'), ('call', 'unix_connect')]:
    args = [mode, socket]
    if mode == 'call':
        args += [Path('examples/localrpc/fixtures/normalize.json').read_text().strip()]
    grant = dict(io=dict(stdout='stdout.bytes@1', dirs=[], net=dict(
        provider='net.stream@1', rules=[dict(address=socket, service='0', operation=operation)])), args=args)
    (root / (mode + '.json')).write_text(json.dumps(grant))
PY
# In one terminal (substitute the directory printed by `printf '%s\n' "$work"`):
timeout 60 hirsel run examples/localrpc/hirsel.json --grants "$work/serve.json"
# In another terminal, after {"listening":true} appears:
timeout 60 hirsel run examples/localrpc/hirsel.json --grants "$work/call.json"
# After both processes exit, rpc.sock has been removed. Remove the grant files:
rm "$work/serve.json" "$work/call.json"
rmdir "$work"
```

`hirsel build --backend llvm examples/localrpc/hirsel.json -o /tmp/localrpc`
(or `--backend cranelift`) builds a standalone executable. Launch with
`timeout 60 /tmp/localrpc --hirsel-grants "$work/call.json" call
"$work/rpc.sock" '{"id":"a","method":"ping"}'`. A run's interpreter arguments
come from grants; a standalone binary takes arguments after its launcher options.
Keep the external deadline: initial accept and a connect waiting for backlog space
have no timeout API. Connected stream reads and writes each have a two-second
inactivity timeout. An external deadline also bounds a peer sending bytes slowly.

The only granted capability is stdout for help. Socket modes additionally require
`net.stream@1` with port/service `0` and precisely `unix_listen` or `unix_connect`
for the chosen canonical absolute path. No directory, stderr, process, clock,
TCP, stale-socket-removal or wildcard grant is requested. Normal and early-error
resource drop closes streams and removes the listener's socket file. An existing
path is an I/O error; the program never requests permission to remove stale files.

The wire format is exactly four big-endian bytes holding a u32 payload byte length,
followed by UTF-8 JSON. Each frame must contain 1–2048 bytes. Length is checked
before any body accumulation. Partial headers/bodies are retained until complete;
EOF in either is an input error. EOF between frames ends a successful session,
including a connection sending zero requests. The eighth response ends the server
without waiting for EOF; any later requests are outside that session.

Request fields are exact and order-independent:

| Method | Fields | Reply |
| --- | --- | --- |
| `ping` | `id`, `method` | `{"id":"…","result":"pong"}` |
| `normalize` | `id`, `method`, `text` | `{"id":"…","result":"…"}` |
| Any other method | `id`, `method` | `{"id":"…","error":{"code":"unknown_method","method":"…"}}` |

IDs are nonempty strings of at most 64 UTF-8 bytes; method names are nonempty
strings of at most 32 bytes. Extra fields, wrong types, duplicate keys and invalid
JSON/UTF-8 are rejected. Text is at most 512 UTF-8 bytes. Normalization trims only
ASCII space and bytes 9–13 at both ends and lowercases only ASCII A–Z. It preserves
internal whitespace and every non-ASCII scalar. Unknown methods produce a typed
response and successful transport status, allowing an agent to choose a fallback.
The client requires the exact expected ID and typed response contents. One reply
completes the call; it does not wait for or consume a second reply.

Input is bounded before parsing: 2048 bytes per frame/JSON argument, four header
bytes, eight requests per connection (at most 16416 wire input bytes). A JSON DOM
can contain at most the tokens fitting that frame and std's nesting limit of 128;
application records have at most three fields. The decoder stores only one bounded
frame, with reads capped at the remaining header/body size. Response and output
builders are bounded by these same field caps. Fixture files are under 4 KiB each;
network fixtures use at most eight records and far less than 64 KiB per cell.

All ordinary outcomes have empty stderr. `call` prints one compact response plus a
newline. `serve` prints `{"listening":true}` after binding, then either
`{"served":N}` or the error record. Output order and JSON field order are stable.

| Exit | Meaning |
| --- | --- |
| 0 | Help, a valid response (including unknown method), or successful session |
| 2 | Typed `model.InputError`, printed as `{"error":"CODE"}` |
| 3 | Missing grant, socket/timeout/output error; prints `{"error":"io"}` when stdout is usable |

Input codes are `frame_limit`, `empty_frame`, `truncated_header`, `truncated_body`,
`utf8`, `json`, `shape`, `text_limit`, `response_mismatch`, and `usage`.
Returning `process.ExitCode` runs normal cleanup. Error frames are not sent for
invalid input: the server closes the connection and reports the local error.

Run the focused integration tests from a prepared checkout:

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=localrpc_23
```

Two tests execute (243 runtime fixture processes in the integration matrix):
pure framing/application vectors under checked H, and actual
CLI interpretation plus LLVM/Cranelift builds and executions. Python `struct` and
`json` provide independent framing and response expectations. Tests check both
modes against Python peers, Hirsel pairs, split/coalesced frames, every EOF
boundary, exact/over limits, malformed inputs, missing grants, deterministic
output/status/stderr and actual socket/file cleanup. [FRICTION.md](FRICTION.md)
records the attempted forms and repairs.

Framing structure and vectors adapt Tokio `tokio-util/src/codec/length_delimited.rs`
lines 504–568 and `tokio-util/tests/length_delimited.rs` lines 94–114, 147–182,
revision `ce685ee6916549ed7e52dbfd5430858bbdc734d3`.
Deviations: a fixed u32 width and zero adjustment replace the configurable builder;
a byte transition and bounded synchronous Net reads replace async `BytesMut` and
reservation; zero length is rejected because it cannot hold JSON; a typed input
error replaces `io::Error`. Coalesced data remains in the socket until the next
bounded read. Raw upstream payloads are exercised directly by the pure decoder,
while socket tests use JSON payloads and an independent Python oracle. The state
machine, binary integers and JSON remain application/stdlib source; no RPC
framework or new protocol standard is introduced.

Upstream license (applies to the adapted structure and vectors):

```text
MIT License

Copyright (c) Tokio Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
