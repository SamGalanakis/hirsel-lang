# archiveaudit

Audit every member of a local, bounded `.tar` or `.zip` without extracting files.
TAR uses std's V7/USTAR/PAX/GNU reader; ZIP uses std's stored/deflate reader and
validates expanded size, CRC (including zero) and data descriptors. Encrypted ZIP
members are rejected; Unix and Darwin mode bits are checked for links/devices.
Compression around TAR is not accepted. Archive suffixes are case sensitive; a malformed ZIP never falls back
to TAR parsing.

From the repository root, grant a read-only directory containing the archive and
manifest, plus stdout. These are all the capabilities used; no write, process,
network, stdin, clock, environment or task grant is needed. Arguments name relative
files in the logical `input` directory. Replace `/absolute/input` below with your
local directory, and save the JSON outside it as `/tmp/archiveaudit-grants.json`:

```json
{"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"input","host":"/absolute/input","access":"read"}]},"args":[]}
```

```sh
hirsel check examples/archiveaudit/hirsel.json
hirsel run examples/archiveaudit/hirsel.json --grants /tmp/archiveaudit-grants.json -- manifest release.tar > /absolute/input/release.json
hirsel run examples/archiveaudit/hirsel.json --grants /tmp/archiveaudit-grants.json -- verify release.tar release.json
# ZIP: use release.zip in the same commands.
# Native: add --native --backend llvm (or cranelift) before --.
hirsel build examples/archiveaudit/hirsel.json --backend llvm -o /tmp/archiveaudit
/tmp/archiveaudit --hirsel-grants /tmp/archiveaudit-grants.json manifest release.zip
```

The program exits **0** for a valid manifest or exact verification match and **1**
for mismatch or invalid input/I/O. This follows Hirsel's `Result` launch contract
(PROG-3); application errors cannot select exit 2. The deterministic JSON
`summary.class` distinguishes `ok`, `mismatch` and `input_error`. Missing stdout
authority or output failure returns `Grant`/`Output` on stderr because a report
cannot be emitted. Hirsel's compiler/launcher can separately refuse a launch.

The version 1 manifest is compact UTF-8 JSON followed by one LF, with fixed key
order and entries sorted by UTF-8 path bytes. Every entry has `path`, `type`
(`file` or `directory`), `size` and lowercase `sha256`. Directories have size 0
and SHA-256 of empty bytes. An empty archive emits exactly:

```json
{"schema":1,"entries":[],"summary":{"class":"ok","members":0,"bytes":0,"missing":[],"extra":[],"changed":[]}}
```

`verify` emits the actual entries and recomputed summary. It compares the exact
path set, type, size and digest; sorted `missing`, `extra`, and `changed` arrays
are separate. The input summary is informational and ignored; it cannot override
any entry. Schema 1 and the three top-level fields are required. Duplicate JSON
keys, duplicate entry identities, unsupported schema versions, unknown entry
fields, invalid sizes/digests/types and unsafe paths fail before reading the
archive. Digests must be 64 lowercase hex characters. Unknown top-level fields
are rejected. Failure reports have this shape, with a stable explicit error:

```json
{"schema":1,"summary":{"class":"input_error","error":"duplicate_path"}}
```

Limits are fixed in `main.hir`: 1,048,576 archive bytes, 131,072 manifest bytes,
128 members, 512 UTF-8 bytes per member path, 65,536 expanded bytes per entry and
262,144 total expanded bytes. Local reads probe at most one byte over the cap.
TAR bodies hash in chunks of at most 4096 bytes; ZIP buffers one bounded member
at a time. ZIP enumeration is bounded by archive bytes before the member count
check; std's TAR special metadata limit is 1 MiB. Inputs are buffered in memory;
these are byte/work bounds, not a fixed RSS guarantee.

Names are preserved. Absolute, empty, dot, parent, repeated-slash, backslash,
colon/drive and NUL names are refused. Directories may have one trailing slash;
`a` and `a/` collide and are rejected together, never merged. Regular files may
not end in a slash. Unicode is not normalized: distinct UTF-8 spellings remain
distinct names. Symlinks, hard links, devices, FIFOs, sparse files and global TAR
metadata entries are rejected for both commands. No member path is opened on the
host. For TAR, std retains Go's EOF convention: an empty file, EOF exactly after
member data, or EOF inside final padding is valid; partial headers and short
member data fail. Trailing bytes after the TAR terminator follow std behavior.
NUL termination inside conventional TAR header fields is part of that format;
PAX path values containing NUL fail.

Record reads and reproduce the report after deleting both archive and manifest:

```sh
hirsel run examples/archiveaudit/hirsel.json --grants /tmp/archiveaudit-grants.json --record /tmp/archiveaudit.tape -- verify release.tar release.json
# Move/remove the inputs, then replay without grants:
hirsel run examples/archiveaudit/hirsel.json --replay /tmp/archiveaudit.tape
hirsel run examples/archiveaudit/hirsel.json --replay /tmp/archiveaudit.tape --native --backend cranelift
```

The independent harness generates archives with Python `tarfile`/`zipfile`, hashes
with `hashlib`, compares exact manifest bytes on H/LLVM/Cranelift, records success,
mismatch and malformed-input paths in each engine, deletes the source directory
and grants file, then replays each tape through all three engines:

```sh
python3 crates/hirsel-cli/tests/support/archiveaudit_15/verify.py --hirsel /absolute/path/to/hirsel --root examples/archiveaudit --output /tmp/archiveaudit-evidence
.buck2/bin/buck2 test //:hirsel-cli-test-cli-shard-1 -- --test-arg=archiveaudit_15
```

Prior art: Go `src/archive/tar/reader.go:44–130`,
`src/archive/tar/reader_test.go:793–855` and
`src/archive/zip/reader_test.go:1656–1681` at
`ebe2feefc400f813aefdc8a036e66d2619da6649` (BSD-3-Clause, The Go Authors;
see `third-party/NOTICES.md`). Walking and truncation/undersize controls adapt
those sources. Codec implementations remain in std; the semantic adaptations
are explicit capabilities and replay, bounded callbacks, strict path/kind and
set checks, SHA-256 manifests and deterministic JSON reports. The oracle/record
harness structure follows `examples/verify-dogfood6.py:18–90` at `7d84d688`.
