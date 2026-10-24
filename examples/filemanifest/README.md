# filemanifest

Select a small workspace's regular files by a Go-style slash glob, print a
reproducible SHA-256 manifest, and detect stale or missing selected artifacts.
The tool reads a live directory and never writes, copies or deletes a file.
Pure path/manifest validation and rendering live in `model.hir`; `main.hir`
acquires capabilities, walks, checks metadata and reads branded file handles.

From a fresh checkout, run from the repository root:

```sh
hirsel check examples/filemanifest/hirsel.json
hirsel run examples/filemanifest/hirsel.json --grants examples/filemanifest/grants.json -- --help
hirsel run examples/filemanifest/hirsel.json --grants examples/filemanifest/grants.json -- scan fixtures '*.txt'
hirsel run examples/filemanifest/hirsel.json --grants examples/filemanifest/grants.json -- scan fixtures 'docs/*.txt'
hirsel run examples/filemanifest/hirsel.json --grants examples/filemanifest/grants.json -- verify fixtures selected.json
hirsel run examples/filemanifest/hirsel.json --grants examples/filemanifest/grants.json -- verify fixtures invalid.json
hirsel build examples/filemanifest/hirsel.json --backend llvm -o /tmp/filemanifest
/tmp/filemanifest --hirsel-grants examples/filemanifest/grants.json verify fixtures selected.json
# Use --backend cranelift for the other native compiler.
```

`ROOT` is a logical directory selector explicitly listed in the grants template;
`fixtures` maps only to the checked-in `fixtures/release` directory. Template host
paths are relative to the template file. For another workspace, copy the template
and replace its host with an absolute directory path. Both commands request only
read access, stdout and stderr. No process, socket, network, clock, environment,
stdin or write capability is used. `--help` needs stdout/stderr but no directory.

`scan ROOT PATTERN` matches the entire relative path. `*` and `?` do not cross
`/`; classes and backslash escaping follow `std/glob` and Go `path.Match`.
`a*` cannot select `a/b`, `a*/b` can select `abc/b`, and `a\*b` selects a literal
star. No recursive `**`, gitignore, shell expansion or archive generation is
added. Quote patterns in a shell. Paths preserve UTF-8 spellings without Unicode
normalization and sort bytewise. For example, `scan fixtures '*.txt'` prints:

```json
{"schema":1,"entries":[{"path":"abc.txt","size":3,"sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"},{"path":"empty.txt","size":0,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}]}
```

Output is compact JSON with fixed field order and one final LF. Empty selection
is `{"schema":1,"entries":[]}` plus LF. `verify ROOT MANIFEST` reads a manifest
relative to ROOT, sorts its entries, and checks exactly those files; unlisted
files are allowed. Verification reports sorted `missing` and `changed` arrays:

```json
{"schema":1,"missing":[],"changed":[]}
```

Removing `docs/notes.txt` and changing `abc.txt` produces
`{"schema":1,"missing":["docs/notes.txt"],"changed":["abc.txt"]}`.
A same-size content change is detected. Manifest schema/field names are strict;
unknown fields, duplicate keys/paths, invalid sizes, non-lowercase SHA-256 digests,
invalid UTF-8 and malformed JSON are refused. All records validate before any
listed file is read. This is an application JSON document, not a new standard.
Invalid input is a typed `model.InvalidInput {kind, path}`, rendered on stderr:

```json
{"class":"invalid_input","kind":"invalid_path","path":"../secrets/token"}
```

That is the checked-in `invalid.json` rejection: an agent can inspect a received
manifest without reading its attempted parent path. Success reports use stdout;
input failures leave stdout empty. Status **0** means scan/help success or all
listed files match; **1** means verification mismatch; **2** means invalid input,
a missing/denied directory, traversal/metadata/open/read failure; **3** means a
missing stdout/stderr grant. An output error propagates through `AppError` and
uses Hirsel's existing status 1/error rendering. Compiler/launcher refusals are
separate from application statuses.

Application limits are inclusive and fixed in `model.hir`:

| Input/collection | Limit |
| --- | ---: |
| Returned walk paths, including nonmatching paths | 128 |
| Relative path components | 16 |
| Relative path UTF-8 bytes (also ROOT and manifest name) | 512 |
| Pattern UTF-8 bytes before compilation | 128 |
| Selected files or manifest records | 64 |
| Bytes per selected regular file | 4096 |
| Total bytes read from selected files | 16384 |
| Manifest input bytes | 65536 |
| Hash read chunk | 1024 |

Checks precede application collection growth. Hashing counts actual bytes and
probes at most one byte beyond a file or total cap, retaining one chunk. The
manifest uses `file.read_all_bounded` and a one-byte excess probe. JSON parsing
is bounded by input bytes before its record-count check; its DOM can exceed 64
nodes, but cannot grow from an unbounded stream. Arguments are supplied by the
launcher; the app rejects wrong counts before collecting them. It does not
retain a copy of an overlong pattern.

`file.walk` reads the root directory at construction and descendant directories
on demand, yielding regular files and symlinks in global UTF-8 byte order.
Directories, including empty ones, are traversal work and do not count toward the
128 yielded-path cap. The app applies `stream.bound(stream.from_results(paths), 128)` before
path validation or matching, then drains it on successful scans. Exactly 128
paths succeeds; a 129th result returns `visited_limit` before validating, matching
or hashing that result. This includes a descendant-read error as the 129th result;
an earlier traversal error returns `walk`. Discovering the extra result can require
directory reads, but no later subtree is read after it. `stream.bound` supplies the terminal overflow check; `take_n(128)` still
silently truncates and cannot implement it. `glob.files` collects its matches, so the
app keeps the `glob.compile/is_match` and demand-pulled `file.walk` composition.

The cursor retains one sorted page per active depth, O(depth × largest single
directory) entries and path bytes. Consumed Path identities and recorded inputs
can remain retained for the run. The app caps yielded paths and retained records;
these caps do not bound directory entries, total traversal depth, memory or RSS.

The conservative application policy refuses a selected symlink or any symlink
component of a selected path, and requires every opened leaf, including the
manifest, to be regular. The walk yields links without resolving their targets or
descending them: unselected dangling, escaping and cyclic links are tolerated.
Verification only examines listed paths. Metadata/open are separate operations,
so concurrent replacement is not an atomic no-follow guarantee; ordinary Dir
descriptor confinement still applies at every lookup and prevents escaping the
grant. The tool does not promise a filesystem snapshot. Special files are refused
without opening their data. Historical limitations and their resolution remain in
[FRICTION.md](FRICTION.md).

The `filemanifest_stream_25` partitions freeze 0/127/128/129-path trees and
cap/lookahead/error controls. The actual package runs under the reference
interpreter, checked H, LLVM and Cranelift against the same independent expected
status and bytes. Scripted simulator effects and native recordings check directory
reads and selected opens, including an unread later failing subtree. Fixtures
adapt walkdir's nested tree and independently sort its raw file/link catalogue;
root/directory emission and default DFS order are explicit adaptations.

The focused acceptance harness uses private temporary trees and compares exact
stdout, stderr and status through interpreter, LLVM and Cranelift. All 30 rows
from Go `path/match_test.go:19–49` run through application selection, with an
extra `a*`/`a/b` case. Empty/abc SHA-256 vectors are checked against upstream
constants, and file digests independently use Python `hashlib`; Python glob is
never used. Checked H also executes the pure model and upstream controls.
Fixture contents, modes, symlinks and an outside-grant sentinel are snapshotted;
cleanup is asserted. Runtime children have a 60 CPU-second/2 GiB budget, and
compilation is setup under normal build limits.

```sh
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=filemanifest_23
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=filemanifest_stream_25
```

Prior art: Go `src/path/match.go:16–65`, `src/path/match_test.go:19–49`,
`src/crypto/sha256/sha256_test.go:20–50` and `sha256.go:52–63`, all at
`ebe2feefc400f813aefdc8a036e66d2619da6649`, Copyright The Go Authors,
BSD-3-Clause ([license](../../third-party/NOTICES.md#bsd-3-clause-license-go)).
Matching and hashing remain shipped std implementations. Deviations are explicit
Dir/File capabilities, typed app errors, bounded chunk updates, stricter relative
path/link policies, JSON manifests and byte-sorted output. The harness structure
follows archiveaudit's include_str inputs and independent Python oracle; this
package reads live files and verifies a selected set without an archive.
