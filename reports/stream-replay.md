# Sequential stream replay fixtures

`crates/hirsel-cli/tests/stream_replay.rs` records real-host pipelines using
both LLVM and Cranelift, then replays each native tape with the interpreter.
Replay receives no grants file or stdin. The file fixture deletes its mounted
directory; the HTTP fixture joins and destroys its loopback server before
replay. Clocks and scheduling are supplied only by the recording. Successful
replay must match every request and consume the complete tape, as well as
produce the asserted stdout and typed outcomes.

The interpreter replay path is `hirsel-driver/src/lib.rs:544`
(`replay_observations` shares the same executor) and
`hirsel-driver/src/network.rs:191-200`: replay dispatches to `SimIo`, with no
`RealIo` provider available to file/network/clock calls. Removing the host
inputs also guards against accidentally launching a live run.

The tests extend the existing `tests/cli/replay.rs` host-removal pattern and
`tests/cli/tape_identity.rs` event-section comparison. No runtime, ABI,
language, serialization or tape encoding change is needed.

| Fixture | Observations |
| --- | --- |
| `stream_replay/file.hir` | One-byte UTF-8 splits; map/filter order; clean EOF; zero-demand and one-item prefixes; sink error; owned file closes and destination finalizer before return. Both native event sections must match. |
| `stream_replay/net.hir` | Owned socket chunks including repeated EOF; complete one-byte UTF-8 splits; a writer accepting only one byte per call must finish suffix lengths 4, 3, 2, 1 before completion; exact write requests/counts and socket cleanup. |
| `stream_replay/http.hir` | Chunked response/trailers; one-byte body outputs; scoped CSV with rejected row, quoted UTF-8/CRLF, first two accepted outputs and a clock-suspending sink; source drop before return; truncation error exactly once followed by end; three network closes; recorded random scheduling with a sibling task. |
| Existing `CONC-3-stream-scoped-cancel-run.hir` | Always-ready records and an incomplete record both stop within one 16-byte refill; typed cancellation, byte-bound assertions and source finalizers precede return. Both native event sections must match. |

Each native recording replays with the same output, including error and
finalizer witnesses. Source edits changing a file chunk limit, CSV refill
limit or explicit check sequence refuse at program binding before output.
Core request matching checks the original chunk limits and actual short-read
bytes; it is not an assertion that separately recorded live network runs have
identical chunking. Comparing native file/cancellation tapes also exercises
the two backends' preservation of pull/effect/destruction order.

## Prior art and adaptations

Inspected upstream: Tokio `ce685ee6916549ed7e52dbfd5430858bbdc734d3`,
local `/tmp/ref-tokio`. `tokio-util/src/io/reader_stream.rs:91-123` performs
one read per demand and fuses error/EOF. The vectors in
`tokio-util/tests/io_reader_stream.rs:39-66` assert data then one error and
end, and `:69-80` assert repeated EOF. Those structures are ported to the
HTTP truncation and clean-end cases; one-byte UTF-8 payloads exercise the
Hirsel byte limit and the HTTP framing boundary independently.

The scoped pipeline and cancellation vectors reuse the prior lane's
[Go CSV and Tokio cooperative-loop ports](stream-scoped-port.md), including
their checked limits and explicit Scope semantics. Effect's pull composition
(`4cddd3c41e71fc4990d37c42b1bcf2ce3780b41d`,
`packages/effect/src/Channel.ts:7040-7043`, local `/tmp/ref-effect`) motivates
the sequential callback order; it introduces no extra task or queue.

Deliberate adaptations: Hirsel uses synchronous owned pulls rather than
Rust Pending/wakers or Effect fibers; typed HTTP truncation replaces Tokio's
raw OS-error vector; existing explicit scoped checks replace Tokio's hidden
cooperative budget. Actual host recording followed by interpreter replay is
Hirsel-specific evidence. The carrier remains the prerequisite's opaque
`stream.Flow` (`Stream` is the core socket handle); the replay lane changes
no API spelling or authority contract.
