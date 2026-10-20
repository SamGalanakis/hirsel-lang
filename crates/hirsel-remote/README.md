# hirsel-remote

The `remote.tls@1` runner provider (docs/spec/runners.md, RUN-13): runner calls
that cross to another host over TLS on a TCP or Unix stream socket, exactly
once per call position, with cancellation, in-doubt outcomes and status
queries. The crate is protocol state machines plus their transport; the driver
wires them to the interpreter (`hirsel-driver` `network/remote.rs` on the
caller, `serve.rs` on the host).

**Frames** (`frame.rs`, `wire.rs`). One frame is one value of the core type
`RunFrame` (`Open`, `Opened`, `Refused`, `Call`, `Outcome`, `Cancel`,
`Acknowledged`, `Status`, `Running`, `Unknown`, `Close`) in the SER-2 canonical
codec, behind an 8-byte little-endian length. Both sides take the codec schema
from their own checked core registry, so a peer whose `RunFrame` differs fails
to decode rather than misreading. `Wire` also computes the RUN-5 request digest
from a `Call` frame's codec bytes, which equals the digest the caller recorded.
`data.rs` is the schema-guided value model the frames convert through.

**Streams** (`host.rs`). Each caller (a TLS identity the host configuration
names) holds one ordered call stream per caller run and runner (STORE-12): the
caller numbers its calls from 1 within an epoch; the host decides positions
strictly in order, each exactly once, and stores every decided outcome until
the caller acknowledges it. A duplicate of a decided position with the same
digest gets the stored outcome; any other duplicate is `Rejected`; a gap is
refused and stays refused until the session reopens. A higher epoch fences the
old session. `Status` answers a stored outcome, `Running` or `Unknown`;
`Cancel` cancels a queued call at once and forwards to a running one. The
position check runs before the RUN-8 code and authority checks, so a refused
call still consumes its position and every position has a stored outcome.

**Client** (`client.rs`). The caller's half: the commit point is the `Call`
frame leaving; a call never resent. A lost connection reconnects up to the
runner entry's `reconnect`; positions the host reports applied are resolved by
`Status`, later ones complete `NotRun(Unreachable())` (retried while `retry`
lasts); against a host that did not keep its streams (another incarnation
without a durable journal) every sent call is `InDoubt(Lost())`. `timeout`
completes a sent call `InDoubt(TimedOut())`.

**Journal** (`journal.rs`). The host's seam for keeping streams across a
crash: `journal.none@1` (memory only; a restarted host is a new incarnation),
`journal.dir@1` (an append-only, checksummed, fsynced file whose torn tail is
cut on recovery; an admitted call without a decision recovers as
`InDoubt(Abandoned())`) and a simulated disk for tests.

**Transport** (`transport.rs`). rustls 0.23 with the ring provider, as the
program-facing TLS provider (`hirsel-rt` `net_tls.rs`): the host presents its
chain and, with client trust anchors, requires and verifies client
certificates (mTLS); the caller verifies the host's name against the webpki
roots or a PEM bundle. Session resumption is off on both sides. Each connection
runs on its own thread.

Tests: `tests/it/sim.rs` drives client and host through a seeded world that
drops, duplicates, reorders and delays frames and crashes the host, and checks
that every call runs at most once and completes exactly once;
`tests/it/journal.rs` covers recovery from torn and corrupt journals.
