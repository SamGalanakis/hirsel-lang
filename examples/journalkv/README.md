# journalkv

A bounded local append-log key-value tool. Exactly **one process must own the
whole database directory** while a command runs. There is no interprocess lock
and no claim that concurrent processes, or outside modifications, are safe.

Build from the repository root with a matching Hirsel CLI/native toolchain:

```sh
hirsel build examples/journalkv/hirsel.json --backend llvm -o /tmp/journalkv
mkdir -p /tmp/journalkv-demo/data /tmp/journalkv-demo/commands
cp examples/journalkv/grants.template.json /tmp/journalkv-demo/grants.json
cp examples/journalkv/fixtures/changes.json /tmp/journalkv-demo/commands/changes.json
/tmp/journalkv --hirsel-grants /tmp/journalkv-demo/grants.json apply changes.json
/tmp/journalkv --hirsel-grants /tmp/journalkv-demo/grants.json get sheep
/tmp/journalkv --hirsel-grants /tmp/journalkv-demo/grants.json scan
/tmp/journalkv --hirsel-grants /tmp/journalkv-demo/grants.json compact
```

The private grants file explicitly mounts `data` for read/write and `commands`
for read. Relative host paths resolve against the grants file. The application
uses no ambient current directory or std.store database. `get KEY`, `scan`,
`apply COMMAND_FILE` and `compact` are the complete CLI. File paths are confined
to the `commands` grant. Keys and values are UTF-8 strings; keys must be nonempty.

A command file is a JSON array:

```json
[["put","sheep","wool"],["put","grass","green"],["delete","grass"]]
```

`apply` prints `OK` for each mutation only after its append and `file.sync`
complete. Batch syntax, limits and projected state are admitted before any
append. A batch is **not atomic**: I/O or output failure can leave an acknowledged
prefix. A failed append/sync has no successful response, but may have written
unacknowledged bytes. Environmental process termination has an unknown outcome;
it is not `Canceled`. Reopen and inspect the log before retrying work.

`get` prints a JSON string or `null`; `scan` prints a JSON object in UTF-8 key
order. Both recover complete records from a torn terminal suffix. `apply` refuses
that suffix with `Torn(offset)` until successful explicit compaction. A complete
bad checksum, record type, payload or interior frame is `Corrupt`, even if it is
the last record; compaction refuses corruption rather than guessing data away.
Domain/limit/I/O errors return exit 1. Language failure 2, launch/replay refusal 3,
budget 5 and environmental stop 6 retain their host classifications.

With an explicit interrupt Signals grant, Ctrl-C cancels snapshot preparation;
the signal waiter and worker are joined before returning Canceled. If that grant
is absent, process termination keeps its ordinary environmental outcome. A
completed publication is never retroactively reported canceled.

Compaction freezes the recovered latest-state snapshot and processes it in one
bounded child, with cancellation checks between records. The writer admits no
writes during that call. It retains latest values **and tombstones**, writes a
replacement AtomicFile, consumes commit, reacquires the published file through
`file.open_append`, and synchronizes it and its directory chain before printing
`COMPACTED`. A dropped precommit AtomicFile aborts its temporary; atomic commit
alone is not a durability promise. Old writer handles leave their scope before
publication. Handles from before a simulated crash are never reused.

| Resource | Inclusive cap |
|---|---:|
| Log including physical padding | 1 MiB |
| Physical mutation records | 256 |
| Live keys | 64 |
| Distinct indexed keys including tombstones | 256 |
| UTF-8 key / value | 128 / 4096 bytes |
| Commands per apply / raw command file | 64 / 1 MiB |
| Concurrent preparation children | 2 |

Compaction is explicit; there is no automatic growth or record-cap increase.
It helps when history has superseded versions. If all 256 records represent
distinct latest keys/tombstones, retaining them leaves no record space for a new
mutation. No tombstone garbage collection or unbounded rollover is promised.
The borrowed JSON pull reader checks command counts before allocating a command
and token lengths before their owned copies; escaped tokens can use bounded std
scratch up to 6 times the decoded byte cap. File reads and physical payloads have
separate caps. Large boundary vectors retain tooling-limit dispositions.

The six explicit manifest modules separate CLI, physical framing, recovery,
generic ordered state, compaction and session admission. The ordered index is
instantiated for recovered string values and prepared encoded byte payloads;
there are no fake generic coverage wrappers.

## Durable format

`data/journal.log` consists of LevelDB-style 32 KiB blocks and 7-byte headers:
masked CRC32C little-endian32, payload length little-endian16, physical type1
(FULL), then payload. CRC covers the type byte and payload. Payloads are the
shipped SER-2 version 1 canonical codec over `record.Mutation` (`Put(String,String)`
then `Delete(String)`); native layouts are never persisted. Envelope/schema
bytes bind the exact package/version/type identity. Maximum payload 8192 bytes;
key/value limits are checked before encoding and again after decoding.

This deliberately uses only FULL records. When one will not fit, zero-pad to
the next block instead of emitting fragments. A zero header introduces an
all-zero remainder; short terminal padding is torn and nonzero padding corrupt.
Do not read these files as arbitrary LevelDB database logs: the payload schema,
FULL-only restriction and strict corruption policy are application-specific.
There is no version migration promise before Hirsel 1.0. Back up the directory
while its owner is stopped before changing tools or schemas.

Prior art: LevelDB `db/log_writer.cc:33–103`, `db/log_reader.cc:189–270`,
`util/crc32c.h:29–41`, `db/log_test.cc:384–419`, and
`db/db_impl.cc:943–987`, commit
`bb74ef739973a70ca9f0d90788a1da89b788c0ad`. License: [LICENSE.leveldb](LICENSE.leveldb).
Deviations are bounded FULL-only padding, the existing SER-2 payload, no
skip-and-salvage after corruption, retained tombstones without LevelDB's
base-level proof, and single-owner AtomicFile publication plus explicit sync.
CRC32C and binary byte order reuse std implementations. Generic insertion-point
search follows Go `src/sort/search.go:58–73`, commit
`ebe2feefc400f813aefdc8a036e66d2619da6649` (BSD-3-Clause, [LICENSE.Go](LICENSE.Go)). Checked indexing, overflow-safe midpoint and explicit string comparison
implement its lower-bound predicate.

## Recording and qualification

Keep recordings outside every granted mutable directory:

```sh
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 \
  prlimit --cpu=60:61 /tmp/journalkv \
  --hirsel-record /tmp/journalkv-apply.tape \
  --hirsel-grants /tmp/journalkv-demo/grants.json apply changes.json
/tmp/journalkv --hirsel-replay /tmp/journalkv-apply.tape
hirsel run examples/journalkv/hirsel.json --replay /tmp/journalkv-apply.tape
```

Native observer/record options precede `--hirsel-grants`; application args follow
its operand. A replay uses the recorded providers, so the original mutable dirs
and command files may be unavailable. Changed semantic code/config or malformed
tapes refuse; no real application file effect is reissued. Qualified engine and
replay dispositions, witnesses, and any remaining INCOMPLETE cells are recorded
in [FRICTION.md](FRICTION.md). No all-engine claim follows from source checking.

The application is 809 handwritten Hirsel lines: main81, record214, recover122,
index100, compact103, session189. Qualification currently has96 small observed
reference/checked-H passes,106 native small/boundary passes,18 native Ordered
width/cancellation passes and372 associated tape replays. Four supplemental
crash/cancellation tests pass on all four semantic engines. Large observed
boundaries and unsupported adapters remain **INCOMPLETE**; see FRICTION for
exact counts, original failures, setup repeats and owner dispositions.
