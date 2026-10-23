# Chunkbackup

A bounded backup tool with content-defined Rabin chunks, SHA256 verification,
immutable durable storage, and no-clobber restore. Requires a matching Hirsel
CLI/native toolchain. It accepts an explicit manifest; it never discovers inputs
by scanning the source tree.

From the repository root, create private directories and a concrete grant file:

```sh
mkdir -p /tmp/chunkbackup-demo/source /tmp/chunkbackup-demo/chunks /tmp/chunkbackup-demo/snapshots /tmp/chunkbackup-demo/target
cp examples/chunkbackup/fixtures/hello.txt examples/chunkbackup/fixtures/input.json /tmp/chunkbackup-demo/source/
cp examples/chunkbackup/grants.template.json /tmp/chunkbackup-demo/grants.json
hirsel run examples/chunkbackup/hirsel.json --grants /tmp/chunkbackup-demo/grants.json
hirsel run examples/chunkbackup/hirsel.json --grants /tmp/chunkbackup-demo/grants.json -- verify daily
hirsel run examples/chunkbackup/hirsel.json --grants /tmp/chunkbackup-demo/grants.json -- restore daily
cmp /tmp/chunkbackup-demo/source/hello.txt /tmp/chunkbackup-demo/target/hello.txt
```

The template's relative hosts resolve beside the private grant file. Its source
mount is read only; chunks, snapshots and target are read/write. Verify needs
only read grants for chunks and snapshots. Restore also needs read/write target.
Backup needs Tasks, stdout and stderr. An optional signals.host@1 grant for
`interrupt` races cancellation against ingestion; the snapshot publishes after
all workers and signal losers have joined. No file or snapshot can grant authority.

Native invocation with the same explicit app arguments:

```sh
hirsel build examples/chunkbackup/hirsel.json --backend llvm -o /tmp/chunkbackup-demo/app
/tmp/chunkbackup-demo/app --hirsel-record /tmp/chunkbackup-demo/backup.tape --hirsel-grants /tmp/chunkbackup-demo/grants.json backup input.json daily 4
/tmp/chunkbackup-demo/app --hirsel-replay /tmp/chunkbackup-demo/backup.tape
```

Tapes must stay outside every granted directory. Record/replay and grants flags
precede application arguments. Use `--backend cranelift` to build that backend.
The grant's `placement.cores` can request 1/2/4 while the CLI's final argument
keeps logical file jobs fixed. Requested width alone proves no physical placement.

The commands are `backup INPUT SNAPSHOT JOBS`, `verify SNAPSHOT`, and
`restore SNAPSHOT`. A successful command writes exactly
`{"files":N,"bytes":N}\n` to stdout. An input/resource/file error exits 2 and
writes `{"error":"tag","path":"path","completed":N}\n` to stderr. Output
grant absence exits 3; a failed output operation follows the normal entry error
contract. Tooling stops and language failures remain separate host outcomes.

Input JSON uses this field order:

```json
[{"path":"hello.txt","size":13,"id":"233b4a30c5f52770cc4e6741af46a1f0e018aacd919e33e873d376ceac745c17"}]
```

The shipped input.json contains the exact digest. To declare other files, compute
the SHA256 of their complete bytes and supply the exact size; source changes or
mismatches refuse. Paths are nonempty UTF-8, relative, <=1024 bytes, <=8 components;
empty/dot/dot-dot components, absolute paths, NUL, backslash, colon, symlinks,
duplicate paths and file/parent collisions refuse. Files may be nested; their
parents must already exist in source. Snapshot names have one component.

Snapshots are strict ordered JSON:

```json
{"format":"chunkbackup1:rabin:3da3358b4dc173:64:256:1024:4096","files":[]}
```

Each file is `{"path":...,"size":N,"id":SHA256,"chunks":[...]}`; each ordered
chunk is `{"offset":N,"length":N,"id":SHA256}`. Offsets must cover the file
exactly, chunk lengths are 1..4096, and empty files have no chunks. The reader
accepts noncanonical content partitions within those bounds and verifies all
bytes against chunk and file digests; backup always produces the fixed Rabin
partition. Field order, format/configuration, digest spelling and all bounds
are validated. Unknown/repeated fields and trailing content refuse. This is an
application format with no restic repository/encryption compatibility.

Limits are 16 files, 32768 bytes/file, 262144 total input bytes, 2048 chunk
references, 262144 manifest bytes, four input workers and four chunks in flight.
Input limits are admitted before reads or worker allocation; metadata, bounded
reads and a separate EOF probe detect changes. Each worker owns a 4096-byte read
window and a <=4096-byte chunk. It moves that chunk to a store task and awaits
its durable result before continuing, so cancellation cannot leave an unbounded
producer. Owned descriptors transfer Bytes into SHA256 String IDs; the generic
file metadata seam serves admitted inputs and snapshot results. At most 1040
references can be produced by admitted source bytes/files; the independent
2048 snapshot cap also bounds external manifests. Manifest encoding checks its
worst-case escaped size before growth. Retained metadata is bounded separately
from file data. The current store retains a bounded <=4096-byte copy while
validating publication; no whole input file is retained by ingestion.

Chunks are named by SHA256 of raw bytes. `file.put_new(Durable)` arbitrates
concurrent duplicate writers atomically; the winner is read and verified.
Every chunk completes durably before the manifest publishes. A failed backup
may leave valid unreferenced chunks. The tool never deletes anything.

Restore expects an empty granted target, or a retry containing only declared
files and their parents. A bounded lazy inventory rejects unrelated entries and
symlinks. It validates the complete manifest, then reconstructs each file within
32768 bytes, verifies every chunk and the whole digest, and publishes with
`file.put_new(Durable)`. Equal existing bytes are an explicit idempotent success;
different bytes refuse. There is no exists-then-rename overwrite sequence.
A later failure leaves earlier completed files in place and reports their count;
restore promises no transaction across files. The count includes confirmed
successes: a provider error after publication may leave a complete file even
when that file is not counted. Directory creation can survive a failed file
publication. Protect grants from concurrent hostile mutation; provider
confinement remains enforced on every call.

The six modules contain 1198 handwritten Hirsel lines: main95, manifest232,
chunk131, store113, snapshot349, restore278. They are explicitly registered in
hirsel.json: main (CLI/grants), manifest
(paths/schema/limits), chunk (rolling state), store (immutable I/O), snapshot
(bounded tasks/publication), restore (validation/reconstruction/summary).

Reference: restic/chunker commit 2e8f53f1dad5dbdcf8aca1dbf1bee7d1fc9d8ce8,
chunker.go:106–249 and chunker_test.go:64–70,189–240. Its BSD-2-Clause license is
retained in LICENSE.chunker. The algorithm uses its supported small custom
256/1024/4096 bounds, 64-byte window and polynomial 0x3DA3358B4DC173. Differences:
owned bounded buffers replace Go Reader slices; linear basis construction replaces
the global mutable table cache while preserving table bytes. The pinned Go
library generated the shipped small oracle vectors; upstream 32MiB fixtures are
not acceptance inputs. Std SHA256 and JSON are reused unchanged.

Run the focused harness (actual full entry, modules, providers and grants):

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=chunk_backup_31
```

The three workflow tests dispatch exact case/engine children with a 60 CPU-s
limit and 2GiB native wrapper limit, retain each child's receipts, and fail if a
child fails or executes zero tests. To run one cell, use host mode and the exact
test name; for example:

```sh
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-cli-test-cli-shard-1 -- --env CHUNK_BACKUP_31_CASE=tiny --env CHUNK_BACKUP_31_ENGINE=checked-h --env CHUNK_BACKUP_31_CAP_CPU=1 --env HIRSEL_MEMORY_LIMIT=2G --test-arg=chunk_backup_31_scripted_reference_checked --test-arg=--nocapture
```

Use `CHUNK_BACKUP_31_RECORDERS=Llvm` or `Cranelift` with
`chunk_backup_31_scripted_native_record_replay` for one native recorder. Add
`CHUNK_BACKUP_31_BOUNDARY=1` for native boundary fixtures; the host boundary test
is `chunk_backup_31_bounds_reference_checked`. Every case declares 20M steps,
or 40M for boundaries, before execution; the default interpreter budget stays 1M.
`CHUNK_BACKUP_31_EVIDENCE_DIR` selects an external receipt directory.

The support `oracle.go` generates vectors using the exact pinned Go library;
in a temporary Go module, run `go get github.com/restic/chunker@2e8f53f1dad5dbdcf8aca1dbf1bee7d1fc9d8ce8`
and `go run oracle.go > vectors.json`. It checks reconstruction, reset and
fragmentation on the small custom-boundary inputs. The support `production.py`
expects an external prepared directory containing copies of the six modules and
a hirsel.json with an absolute std dependency. Invoke it with that directory,
a matching CLI, and the absolute tools/capped path; it archives and removes its
own granted directories before replay. Choose `CHUNK_BACKUP_MODES` and
`CHUNK_BACKUP_WIDTHS` to select only missing identities. Its cancellation receipt
is incomplete unless a signal was actually delivered after the handshake.

FRICTION.md records exact engine/replay dispositions and remaining obligations.
A green harness that retains tooling/provider stops does not mean those cells
passed. No all-engine or replay-complete claim is made while any required cell
is incomplete. Free scheduling is off; there is no performance claim.
