# Durable work queue

One finite Unix-socket service owns a private append log. Consumer groups have independent pending and acknowledged deliveries. Claims and acknowledgments synchronize storage before replying. A lease expiration or restart can deliver the same message again, so consumers must tolerate duplicate processing. Every delivery has a new global token. A stale token cannot acknowledge the new owner; repeating the current acknowledged token returns `acked:0`.

Build from the repository root:

```sh
hirsel build examples/workqueue/hirsel.json --backend llvm -o /tmp/workqueue
mkdir -p examples/workqueue/data
/tmp/workqueue --hirsel-grants examples/workqueue/grants-serve.json serve /tmp/workqueue.sock 10000 100
```

The grants mount `examples/workqueue/data` (relative to the template), and authorize just `/tmp/workqueue.sock`. Use a private directory and socket path for each independent queue. Adjust both the template and command together. One process must own the directory; there is no interprocess locking or replication. The example service stops after ten seconds or an explicit stop, and joins its tasks. Restart the same command to reuse its durable queue.

Clients can send multiple requests on one connection. `call` sends one request and prints its JSON response. Four connections are allowed over a service's lifetime; the CLI calls below therefore require restarting the service between sets of four, or using a persistent client the supplied `python3 examples/workqueue/fixtures-client.py /tmp/workqueue.sock` (one JSON request per input line). There is at most one outstanding request or reply per admitted client: each worker waits until its previous response has been written before reading another frame.

```sh
/tmp/workqueue --hirsel-grants examples/workqueue/grants-call.json call /tmp/workqueue.sock '{"op":"group","group":0}'
/tmp/workqueue --hirsel-grants examples/workqueue/grants-call.json call /tmp/workqueue.sock '{"op":"enqueue","data":"send invoice"}'
/tmp/workqueue --hirsel-grants examples/workqueue/grants-call.json call /tmp/workqueue.sock '{"op":"claim","group":0,"consumer":0}'
/tmp/workqueue --hirsel-grants examples/workqueue/grants-call.json call /tmp/workqueue.sock '{"op":"ack","group":0,"id":1,"token":1}'
```

The public request schema has exactly these fields (unknown fields are refused):

| op | fields | response |
|---|---|---|
| enqueue | data: UTF-8 string | enqueued: monotonic id |
| group | group: 0 or 1 | group |
| claim | group, consumer: 0..3 | id,group,consumer,token,data; or empty:true |
| ack | group,id,token | acked:1 on first ack, acked:0 for a matching repeat |
| pending | group | pending rows sorted by id, each id,consumer,token |
| reclaim | group,id,consumer | a new delivery; error:lease before expiration |
| checkpoint | none | checkpointed:true after publication and synchronization |
| stop | none | stopped:true, then admission stops and workers join |

Each group starts at message1, including messages enqueued before group creation. Claim first offers the oldest expired pending delivery, then the oldest never-delivered message. Pending ownership is not removed by time alone. Reclaim requires that exact item to be pending and expired. Restart makes all pending leases expired because process-relative clock origins do not survive. Acknowledge by group, message and current token; tokens are stale-owner guards, not authentication secrets. The acknowledged current token is retained for idempotence. There is no destructive trimming: at128 retained messages a new enqueue is refused. Checkpoint reduces log history, without removing messages or acknowledgment state.

Bounds are checked before reads or admission: 2 groups,4 fixed consumer IDs/group,128 messages,1024 UTF-8 bytes/message,16 pending/group,512 records,524288 log bytes,4 lifetime connections,512 requests/connection,8192 bytes/frame. Session1..10000ms, lease1..2000ms, socket read/write timeout2s. A state-capacity refusal leaves the service usable for pending, ack, checkpoint or stop. A frame-limit violation closes that peer before allocating its body. Exhausting lifetime connections or per-connection requests closes admissions; restart to admit another session. A file append/sync failure stops the service without a success reply. Its disk outcome can be ambiguous and is resolved by restart. CLI usage/resource refusal exits2; fatal storage/network/grant failure or corruption exits3; completed service exits0. Domain refusals are framed JSON responses on a successful transport.

`main` owns CLI/grant projections; `record` owns strict framing, CRC and bounded decoding; `log` owns append/sync and recovery; `groups` owns pure admission and generic bounded payload tables; `dispatch` owns bounded channels, client tasks and finite deadlines; `snapshot` owns frozen read-only checkpoint preparation and deterministic inspection. `groups.Table[String]` retains payloads, while `groups.Table[Bytes]` holds prepared checkpoint records. Both use the same consuming generic insertion function. Capability orchestration stays outside pure state decisions.

## Private format, version1

`queue.log` starts with eight bytes `57 51 33 31 01 00 00 00`. Every subsequent frame is a seven-byte LevelDB FULL header: little-endian masked CRC32C (4), payload length(2), physical type1(1), followed by the payload. CRC covers physical type1 then payload, masked as `(crc>>15 | crc<<17)+0xa282ead8` modulo32bits. Shipped `std@1/crc32c` computes CRC; no CRC implementation is copied into the application.

Payloads are fixed-layout, checked before allocating strings:

* Message: tag1:u8, id:u64LE, byte length:u16LE, UTF-8 data (0..1024).
* Group: tag2:u8, group:u8.
* Delivery: tag3:u8, group:u8, id:u64LE, consumer:u8, token:u64LE, acknowledged:u8 (0 or1).

A checkpoint is a replacement log: messages by ID, then groups by ID and their latest delivery/ack rows by message ID. At most386 records. An atomic temporary is written and committed, then its replacement inode is reopened and synchronized. No append handle survives replacement. Rename/commit alone is never called durable. Recovery rejects complete invalid frames and semantic IDs, and discards only an incomplete last header/body whose available tag and embedded length agree with its advertised extent. A damaged interior length cannot hide following complete records as a short tail. Before appending after a torn tail it publishes and synchronizes the verified prefix. Complete CRC damage is an error even at EOF. A crash after durable claim but before reply leaves a pending item; restart issues a fresh token when redelivering it.

References: BSD Redis7.2 `d2c8a4b91e8c0e6aefd1f5bc0bf582cddbe046b7`, `src/t_stream.c:2820–2888`, `tests/unit/type/stream-cgroups.tcl:47–150`; BSD LevelDB `bb74ef739973a70ca9f0d90788a1da89b788c0ad`, `db/log_writer.cc:34–105`, `db/log_test.cc:374–419`. Licenses accompany the sources. Deviations: integer group/consumer IDs and bounded vectors replace names/radix trees; tokens strengthen Redis XACK; strict error-on-corruption replaces LevelDB's skip-and-continue; bounded FULL frames replace block fragmentation/padding; a single synced replacement log avoids a two-file checkpoint commit. This is not Redis protocol or storage compatibility. Local framing follows the existing localrpc four-byte big-endian length recipe.

See FRICTION.md for exact executed qualifications and remaining obligations. External matrix/oracle/receipts are in `/workspace/code/hirsel-briefs/durable-queue-31/`; no all-engine completion is implied by static checking.

Run focused tests with `python3 tools/buck2/prepare.py`, `python3 tools/buck2/sync.py --if-stale`, then `.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=durable_queue_31`. Archival replay/control audits are explicit ignored tests using the retained evidence environment documented in FRICTION.md.

Tokio framing prior-art terms:

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
