# rpcbatch

A bounded Unix-socket client for agents dispatching read-only Ping/Normalize jobs
and collecting every outcome in input order. It also provides a finite helper
server whose initial idle accept is canceled by a language scope deadline.

```sh
hirsel run examples/rpcbatch/hirsel.json --grants examples/rpcbatch/grants.json
hirsel build --backend llvm examples/rpcbatch/hirsel.json -o /tmp/rpcbatch
# Or: --backend cranelift
```

The template prints help. Real runs grant stdout, cooperative Tasks, a monotonic
clock, and one exact Unix socket operation. Create grants in a private directory:

```sh
work=$(mktemp -d /tmp/rpcbatch.XXXXXX)
python3 - "$work" <<'PY'
import json, sys
from pathlib import Path
root = Path(sys.argv[1])
socket = str(root / 'rpc.sock')
requests = ['{"id":"a","method":"ping"}',
            Path('examples/rpcbatch/fixtures/normalize.json').read_text().strip()]
for mode, op, args in [
    ('serve', 'unix_listen', ['serve', socket, '2', '2000']),
    ('batch', 'unix_connect', ['batch', socket, '2', '2000', *requests]),
]:
    grant = dict(args=args, io=dict(stdout='stdout.bytes@1',
        tasks='tasks.cooperative@1', monotonic_clock='clock.monotonic.ns@1',
        dirs=[], net=dict(provider='net.stream@1', rules=[
            dict(address=socket, service='0', operation=op)])),
        placement=dict(cores=2, commit='shared', reorder='auto'))
    (root / (mode + '.json')).write_text(json.dumps(grant))
print(root)
PY
```

Run the built tool with a readiness handshake:

```sh
python3 - "$work" <<'PYTHON'
import json, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1])
def command(mode):
    grant = root / (mode + '.json')
    args = json.loads(grant.read_text())['args']
    return ['/tmp/rpcbatch', '--hirsel-grants', str(grant), *args]
server = subprocess.Popen(command('serve'), stdout=subprocess.PIPE)
try:
    assert server.stdout.readline() == b'{"listening":true}\n'
    subprocess.run(command('batch'), check=True, timeout=5)
    assert server.wait(timeout=5) == 0
    print(server.stdout.read().decode(), end='')
finally:
    if server.poll() is None:
        server.kill()
        server.wait()
# rpc.sock was removed by listener drop; only our grant files remain.
for mode in ('serve', 'batch'):
    (root / (mode + '.json')).unlink()
root.rmdir()
PYTHON
```

A built executable accepts `--hirsel-grants FILE` followed by application
arguments. These replace the template's Args. Its server deadline starts before
bind and applies to the complete finite session.

`batch SOCKET SLOTS DEADLINE_MS [REQUEST ...]` admits 0..16 requests. Each JSON
argument is indexed from zero; IDs must be distinct. Slots are 1..4 and the
absolute per-call deadline is 1..2000 milliseconds. The whole input is validated
before requesting Tasks, Clock or Net. An empty batch prints `[]` with stdout
alone. Each child starts one connection, sends one request frame, half-closes its
write side, checks one response, closes the connection, and returns its outcome.
There are no retries. Fixed waves contain at most SLOTS children; the next wave
starts after all members of the current wave have joined. A slow member can leave
other slots idle. At slots=1 the maximum waiting time is 32 seconds for 16 calls,
plus bounded validation, encoding and scheduler overhead.

A child returns `Ok(outcome)` even for a domain failure. Thus `task.all` retains
every result in add order instead of canceling siblings on a peer error. Output
is one compact JSON array, with `index`, `id`, `status`, and a canonical `response`
for successful transport, followed by a newline:

```json
[{"index":0,"id":"a","status":"ok","response":{"id":"a","result":"pong"}}]
```

Unknown methods have status `ok` and the localrpc `unknown_method` error response.
Protocol failures use localrpc's `frame_limit`, `empty_frame`, `truncated_header`,
`truncated_body`, `utf8`, `json`, `shape`, `text_limit`, or `response_mismatch`.
Connection/grant/provider errors inside a call use `io`; scope expiry uses
`deadline`. Batch exit is 0 when it can print every terminal row, including failed
rows. Validation errors print `{"error":"CODE"}` and exit 2 (`usage`,
`duplicate_id`, `request_limit`, or a localrpc input code). Missing capabilities
or output errors exit 3, printing `{"error":"io"}` when stdout is usable.

`serve SOCKET LIMIT DEADLINE_MS` accepts 1..16 connections and serves one request
per connection, sequentially. It prints `{"listening":true}` only after bind.
The one absolute scope deadline bounds initial accept, subsequent accepts, reads,
and writes. Its final record is `{"served":N,"expired":BOOL}`. Expiry returns
exit 0 after cancellation and cleanup; a protocol error exits 2, a network error
3. Count includes only fully written replies. Unrelated paths are never unlinked:
the tool asks only for `unix_listen`, never stale-removal authority, and relies on
std's identity-protected listener cleanup. Existing regular files remain intact.

The protocol is exactly [localrpc](../localrpc/README.md): a four-byte big-endian
u32 length and 1..2048 UTF-8 JSON bytes, with the same ID/method/text bounds and
ASCII-only trim/lowercase policy. This package depends on localrpc's model;
its shipped sequential entry is unchanged. Reads stop at the remaining header or
body length, rejecting oversized lengths before body accumulation. EOF before a
reply is `truncated_header`; partial bodies are `truncated_body`. No directory,
remote-network, HTTP, process or stale-path grant is needed.

Recording and replay use the ordinary launcher. Observer flags on a standalone
executable are prefixes in this exact order:

```sh
/tmp/rpcbatch --hirsel-trace-events /tmp/rpcbatch.events.jsonl \
  --hirsel-record /tmp/rpcbatch.tape --hirsel-grants "$work/batch.json" \
  batch "$work/rpc.sock" 1 2000 '{"id":"a","method":"ping"}'
/tmp/rpcbatch --hirsel-replay /tmp/rpcbatch.tape
```

Use fresh output paths and keep recordings private, since they retain payloads.
Replays require no live socket. Physical width is `placement.cores` (or the CLI's
`--cores`), not a change to slots or requests. Free execution is not used.

The permanent tests use independent literal frames and expected JSON for the
fourteen named workflows and their small boundary variants. Interpreter and
checked H run the actual application and compare full observations. Both native
backends run the same scripts, record and replay them, plus real peer controls
for reverse completion, EOF/cleanup, idle accept and existing-path protection.
Native trace export is separately blocked by the runtime owner
`e1-trace-record-27` (`incomplete canonical event evidence`); recording-only
correctness does not claim trace-export qualification.
An exhausted `net.script@2` listener returns `ConnectionAborted`, so a genuinely
pending idle accept has no scripted fixture: that checked-H cell remains
INCOMPLETE; real-socket tests supplement it. Source check or pure-model tests are
never counted as application runtime coverage.

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- \
  --test-arg=rpcbatch_27
```

Prior art: Go `io/io.go:321–355`, `encoding/binary/binary.go:258–290`,
`encoding/binary/binary_test.go:541–568`, `net/unixsock.go:209–231`, and
`net/unixsock_test.go:484–555` at `ebe2feefc400f813aefdc8a036e66d2619da6649`
(BSD-3-Clause, The Go Authors); Tokio `tokio/src/task/join_set.rs:374–395`,
`tokio/tests/task_join_set.rs:267–295`, and
`tokio-util/src/codec/length_delimited.rs:504–568` at
`ce685ee6916549ed7e52dbfd5430858bbdc734d3` (MIT, Tokio contributors).
These inform exact-length/EOF vectors, cap-before-body framing and cancel/join
structure. Hirsel adaptations: std supplies owned branded handles and shielded
cleanup; localrpc supplies the decoder; a single capped frame replaces general
stream codecs; `task.all` returns add order instead of Tokio completion order;
domain failures are data, not panics; scope timers replace explicit abort/join
loops. Tests use request readiness/EOF handshakes instead of Tokio's sleep-based
pending-task setup. Reference licenses are in [third-party notices](../../third-party/NOTICES.md).
