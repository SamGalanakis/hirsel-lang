# s3@1

A pure Hirsel package for S3-compatible object storage, using std HTTP/TLS over
an explicit Net and WallClock. SigV4 uses std sha256/hmac; there are no runtime
dependencies or ambient credentials. The store engine stays in std@1/store.

`sign` is pure, exposes the canonical request/string-to-sign for verification,
and signs every supplied header. `canonical_query` sorts encoded key/value
pairs without dropping duplicates; `encode_path` preserves slash boundaries.
The vectors are AWS's published aws4_testsuite from botocore. Each full
signature exceeds the corpus step budget, so a bounded Rust integration test
executes their unchanged Hirsel assertions in `tests/aws-*.hir`.

`client` binds explicit Config/Credentials/Net/WallClock values. `from_grants`
loads the `HIRSEL_S3_<NAME>_*` Env snapshots expanded from a RunConfig
`object_stores` entry, reads a credentials JSON file through its granted Dir,
and obtains the explicitly granted network and wall clock. Credentials JSON:
`{"access_key":"...","secret_key":"...","session_token":"..."}`; the token
may be omitted. Config carries the endpoint origin, bucket, signing region,
object prefix, cache byte bound and maximum object size. HTTP redirects and
response decompression are disabled for signed requests.

`get`, `get_range`, `put(Create | Match(etag))`, `list_objects` and `delete`
implement GET, validated Range GET, conditional PUT, paginated ListObjectsV2
and idempotent DELETE. Failed requests are not retried. LIST requests URL
encoding, decodes keys, rejects keys outside the prefix, and returns sorted
unique keys. Multipart is unnecessary for the bounded commit objects.

`object_state` and `object_backend` implement store.Backend[ObjectBackend].
Open with `store.open_backend(s3.object_state(client)?, s3.object_backend(),
store.default_config())`. Store handles uncertain creates by comparing the
exact bytes read back; a different object fences the writer. Store retains
all manifest, segment, compaction and durability logic. The object backend
first probes conditional-create support, refusing a server that ignores the header
with `ConditionalUnsupported(endpoint)`. Raw GET/LIST remain usable directly.
The probe uses a timestamped scratch object and leaves a colliding existing key alone.
It uses range reads and an optional bounded cache (0 disables it), invalidated
on creates, deletes and discovery. S3 acknowledges every create durably even
when the engine asks for Volatile.

`simulated(dir, latency, fail_at, lost_reply_at, race_at)` supplies the same
backend over a Test Dir. Ordinals are deterministic backend operations; 0
means off. A race installs different bytes before conditional creation. A
lost reply writes the complete object before failing. Virtual latency adds
`latency` ticks per operation without sleeping. The original db-local corpus
has matching simulator variants, including every crash/compaction boundary.

Protocol references read from shallow clones (no server code copied):

- botocore `4d9f375e71c1a97693d961cdefb1827bdaaccfb9`,
  `botocore/auth.py:273` (encoded query-pair sort), `:304` (canonical headers),
  `:373` (canonical request), `:408` (string-to-sign), `:420` (HMAC derivation),
  `:541` (S3 path preservation); the signer ports this structure.

- Garage `ef6a03a4215e9a545945e6e34b43a185dd7b718a`,
  `src/api/common/signature/payload.rs:307` (canonical path/query/headers),
  `src/api/s3/get.rs:817` (GET preconditions), `src/api/s3/list.rs` (LIST v2).
- RustFS `f4f1743d04045528e2c97897febd87c7e26838d4`,
  `crates/e2e_test/src/conditional_write_policy_test.rs:163` (conditional create),
  `rustfs/src/server/layer.rs:1101` (PreconditionFailed responses).
- arrow-rs object_store `279572ea60f3a7a6e5237e066a6f4a35eee611e0`,
  `src/aws/mod.rs:255` (refuse unsupported conditional PUT; map 304/412 to Exists).
- SlateDB `a5c4fd22b36c836a89d8c4f27bf0b7b045e62e32`,
  `slatedb/src/wal/slatedb/store.rs:225` (create-only log objects),
  `slatedb/src/retrying_object_store.rs:290` (verify uncertain completion).

MinIO is not used: its upstream README, “Source-Only Distribution”, states that
community precompiled releases are no longer provided and legacy binaries
receive no updates. Real-server validation uses Garage or RustFS in scratch.

The standalone `e2e/hirsel.json` program loads a named `data` object store from
`--grants`, reports one acknowledged commit group's elapsed nanoseconds, then
compacts, reopens and verifies the value. Grant stdout and a read-only credentials
Dir alongside the `object_stores` declaration (see docs/spec/determinism.md).
Server contract probes against scratch release binaries found RustFS 1.0.3 returns
200 then 412 for competing create-only PUTs and retains the first bytes. Garage
2.4.1 returns 200 twice and replaces the bytes; the backend must refuse its open.

Differences from botocore are explicit Hirsel inputs in place of ambient SDK
credentials, clock and request state; the caller supplies only stable headers,
which are all signed. Bounded byte payloads always use their SHA-256, so there
is no streaming checksum or unsigned-payload branch. HTTP effects use Net and
WallClock grants and never automatically retry, keeping authority and replay
visible to the caller. The store resolves uncertain creates through its own
read-back protocol. LIST needs only a bounded scan of the URL-encoded S3 fields
rather than the SDK's general XML model decoder; malformed required fields refuse.

Known engine/checker findings are tracked separately: concurrent openers that
finish discovery before either publishes can create identical epoch claims,
so the inherited fence protocol needs a unique per-attempt ownership token.
The generic callback authority regression is retained as a rejected corpus twin
and listed in tools/known-failures.tsv until the source checker fixes it.
Neither issue is resolved by the provider or the conditional-support probe.
