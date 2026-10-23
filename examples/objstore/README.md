# Immutable content-addressed object store

A bounded artifact store in six Hirsel modules. `put` imports a blob, `get`
returns raw blob bytes, `snapshot` imports an explicit manifest and publishes an
immutable root name, `list` prints that root tree's entries, and `verify` checks
the reachable graph. Exit0 means success,2 is an application refusal with a
JSON `kind`/`path` diagnostic on stderr,3 is a missing output grant. Tooling
limits and language failures have their launcher's separate dispositions.

From the repository root, with a matching `hirsel` CLI/native toolchain on PATH:

```sh
HIRSEL="$(command -v hirsel)"
OBJSTORE_DEMO="$(mktemp -d)"
mkdir "$OBJSTORE_DEMO/source" "$OBJSTORE_DEMO/objects" "$OBJSTORE_DEMO/roots"
cp examples/objstore/fixtures/hello "$OBJSTORE_DEMO/source/hello"
cp examples/objstore/fixtures/manifest.json "$OBJSTORE_DEMO/source/manifest.json"
python3 - "$OBJSTORE_DEMO" <<'PY'
import json,sys
from pathlib import Path
root=Path(sys.argv[1])
grant=json.loads(Path('examples/objstore/grants.template.json').read_text())
for mount in grant['io']['dirs']:
    mount['host']=str(root/mount['path'])
(root/'grants.json').write_text(json.dumps(grant))
PY
"$HIRSEL" build examples/objstore/hirsel.json --backend llvm -o "$OBJSTORE_DEMO/objstore"
"$OBJSTORE_DEMO/objstore" --hirsel-grants "$OBJSTORE_DEMO/grants.json" put hello
"$OBJSTORE_DEMO/objstore" --hirsel-record "$OBJSTORE_DEMO/snapshot.tape" --hirsel-grants "$OBJSTORE_DEMO/grants.json" snapshot manifest.json first 4
"$OBJSTORE_DEMO/objstore" --hirsel-grants "$OBJSTORE_DEMO/grants.json" list first
"$OBJSTORE_DEMO/objstore" --hirsel-grants "$OBJSTORE_DEMO/grants.json" verify first
"$OBJSTORE_DEMO/objstore" --hirsel-grants "$OBJSTORE_DEMO/grants.json" get 1e3b6c04d2eeb2b3e45c8a330445404c0b7cc7b257e2b097167d26f5230090c4
"$OBJSTORE_DEMO/objstore" --hirsel-replay "$OBJSTORE_DEMO/snapshot.tape"
```

Keep recordings outside all mutable mounts. Read-only operations need read
access to objects/roots; only put/snapshot need source. Only snapshot needs
Tasks. `grants.read.template.json` provides a read-only template for get/list/verify. Optional `io.signals: {"provider":"signals.host@1","kinds":["interrupt"]}`
makes snapshot cancel and join its children on Interrupt. Publication begins
only after the signal race is resolved and its loser joined. Root names are
immutable: repeating an identical snapshot name is idempotent; changing its ID
returns `root_exists`. Choose a new name for a new snapshot.

The manifest is a flat UTF8 JSON array of records with exactly these fields,
in this order: `{"path":"dir/file","size":3,"id":"64 lowercase hex digits"}`.
Whitespace is permitted. Parents are inferred; empty snapshots are supported.
Paths must already be canonical relative slash paths: no empty/dot/dot-dot
components, absolute paths, backslashes, colons, NULs, symlink components or
file/directory collisions. Duplicate paths fail admission. Case and Unicode
normalization are byte-sensitive; aliases rejected by the canonical-path rule
cannot enter the index. The expected ID is SHA256 of framed bytes, not a raw
file SHA256. This policy detects same-size content changes relative to the
manifest, short/long reads and changed metadata. It is not a filesystem lock;
host confinement is still required against adversarial replacement between
metadata/open calls.

Objects contain exactly `type SP decimal-byte-length NUL payload`, hashed in
full with SHA256. Filenames are the lowercase hex digest. Blobs contain raw
bytes. Trees contain a JSON array sorted by UTF8 name, each row encoded exactly
as `{"type":"blob or tree","name":"component","id":"digest"}` with compact
punctuation, std.json escaping and no trailing newline. Noncanonical trees
are refused. Identical bytes under different types have different IDs.
All reads check size, type, framing and digest before decoding. Even successful
put_new duplicate winners are reread and verified. Every object insertion and
final root-ID file (64hex plus newline) uses Durable put_new. A failed snapshot
leaves newly inserted objects valid and unreachable and never a newly advertised partial root.
No global scan, deletion, GC, checkout or mutable-reference protocol exists.

Bounds:32 manifest files,16KiB/blob,128KiB aggregate input,128 objects,
8 tree levels (root at0),32 entries/tree,1KiB UTF8 relative paths/names,
1–4 jobs and256KiB manifest/index state. Index/object admission is conservative
before deduplication, so a heavily aliased manifest may refuse even when its
unique object graph is smaller. Tree encoding reserves worst-case6x escaped
names; the encoded tree limit is200000 bytes and stored-object limit200032.
Traversal retains a bounded visited table and256-entry DFS frontier. Repeated
DAG references retain greatest visited depth so aliases cannot hide depth
violations. Physical filesystem component limits may impose smaller names.

`object` owns typed IDs/codec dictionaries and framing; `store` owns authority
and verified durable IO; `tree` owns canonical graph encoding; `ingest` owns
Shared manifest access and bounded TaskSets; `snapshot` owns admission, graph
construction and traversal; `main` owns CLI/grant projections. Generic
Codec/Id/load/insert seams are instantiated for Bytes and tree.Node. Byte
buffers move into durable writes; ownership-required copies stay bounded.

Run the focused harness after `python3 tools/buck2/prepare.py`:

```sh
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=object_store_30_scripted_reference_checked
```

Boundary fixtures and retained-tape audits are explicit ignored tests; filter
identities using executor `--env OBJECT_STORE_30_CASE=...`. See FRICTION.md for
actual execution counts, original tooling/failure dispositions and remaining
incomplete obligations. REFERENCES.md records Git adaptation/licensing and
semantic deviations. No performance claim is made.
