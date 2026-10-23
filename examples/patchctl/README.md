# patchctl

Generate a reviewable unified patch or validate its application without changing
any source file. An agent can review a proposed configuration change and check
it against the current file before publishing the result elsewhere.

From this directory, with the built `hirsel` on PATH:

```sh
hirsel check hirsel.json
hirsel run hirsel.json --grants grants.json
hirsel run hirsel.json --grants grants.json -- apply old.txt review.patch
hirsel run hirsel.json --grants grants.json -- apply old.txt stale.patch
hirsel run hirsel.json --grants grants.json -- --help
hirsel build hirsel.json --backend llvm -o /tmp/patchctl-llvm
/tmp/patchctl-llvm --hirsel-grants grants.json apply old.txt review.patch
hirsel build hirsel.json --backend cranelift -o /tmp/patchctl-cranelift
/tmp/patchctl-cranelift --hirsel-grants grants.json --help
```

Native builds need the standard pinned native toolchain, link kit and runtime
archives described in [the build guide](../../tools/buck2/README.md). Explicit
arguments override the snapshot in `grants.json`. The checked-in grant allows
only reads under `fixtures/`, stdout and stderr. No write, process, socket or
clock grant is requested. Arguments are UTF-8 leaf filenames, at most 255 bytes;
empty names, `.`/`..`, separators, colons and ASCII controls are rejected.

`diff OLD NEW` writes a single-file unified patch with fixed display labels
`old` and `new`, Myers and three context lines. Identical inputs produce empty
stdout. `apply SOURCE PATCH` writes the complete resulting text. Patch header
names are display labels: even `/display/only` and `../../never-opened` in
`review.patch` authorize no path access. Empty patches are identity operations.
LF, CRLF and missing final LF remain byte exact. Both files must be UTF-8.

The application accepts an empty patch or a `--- ` / `+++ ` header pair followed
immediately by `@@ -start[,count] +start[,count] @@` hunks (LF header endings,
no function suffix). Std handles the hunk bodies, missing-newline markers and
exact context matching. It may relocate a hunk to matching context with no fuzz,
as documented by [diff.apply](../../docs/spec/std-library.md#line-diffs-and-patches).
Multi-file patches and arbitrary preambles are rejected. All rejected hunk
indexes are reported in patch order, starting at zero. A partial std result
produces **no stdout**, including when an earlier hunk succeeded.

Bounds apply before splitting or accumulating records:

- Each input: 65,536 bytes, read with `file.read_all_bounded` and one EOF probe.
- Source and target: 64 LF-delimited lines, including an unterminated last line.
- Patch: 256 lines, 64 hunks; header positions and counts at most 64.
- Each hunk header: four space-separated fields; each range: at most two fields.
- Generated patches must fit the same patch limits. A large replacement whose
  patch exceeds 65,536 bytes fails, even when both texts individually fit.
- Std may temporarily construct a result bounded by the two input byte budgets;
  successful output must again fit 65,536 bytes and 64 text lines.

Exit status is 0 for success/help, 1 for rejected hunks, and 2 for invalid input,
missing grants or I/O errors. Errors use the typed `model.InvalidInput` and
`AppError` variants, rendered as one deterministic stderr line, for example
`patchctl: rejected_hunks [0]` or `patchctl: malformed_patch line=4`.
Without stderr authority the same failure status is returned silently. Output
write errors return 2; an OS write can itself be partial.

`model.hir` holds pure validation/transformation and typed reports; `main.hir`
holds argument access, bounded reads, output and process statuses. The manifest
explicitly registers both. See [FRICTION.md](FRICTION.md) for the exercised forms.

The focused Rust harness embeds both sources, the manifest and all four fixtures
as declared build inputs. It checks 15 input pairs (including imara-diff's
replace and identical-file vectors), independently applies generated patches,
and tests authored patches, partial rejection, limits, grants and cleanup on
interpreter, LLVM and Cranelift. A separate checked H test exercises the pure
model and audits zero live heap bytes. Run:

```sh
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=patchctl_23
```

Algorithms remain in shipped std. Reference usage and vectors:
[pascalkuthe/imara-diff](https://github.com/pascalkuthe/imara-diff/tree/f02e46a737c43c0d9c65a42e5e08046bd10fd739),
`src/myers.rs:1-45` and `tests/integration/main.rs:26-94`, Apache-2.0,
Copyright Pascal Kuthe and imara-diff contributors; license and attribution in
[third-party notices](../../third-party/NOTICES.md). The input vectors are
retained; this application's test permits any valid edit script, adds file
headers, and fixes Myers/context=3 instead of iterating all algorithms.

## Tree changes

The single-file commands above keep their original behavior. Tree commands use
separate `source`, `target`, `fixtures` and `output` directory grants:

```sh
cd examples/patchctl
mkdir -p tree-source tree-target tree-output
hirsel run hirsel.json --grants grants-tree.json -- tree-diff > fixtures/proposed.json
hirsel run hirsel.json --grants grants-tree.json -- tree-check proposed.json 4
hirsel run hirsel.json --grants grants-tree.json -- tree-apply tree-change.json 4
```

The last command adds `new.txt` containing `hello` and LF to a separate output
tree. `tree-output` must initially be empty, including empty directories. Use a
fresh empty output directory for every application. `grants-tree.json` is a
relative-path template; private copies may name absolute host roots. Source and
target grants stay read only. A check requires no output grant. The optional
fourth operand is a deadline in milliseconds, 0..30000; it requires the granted
monotonic clock. Zero requests immediate cancellation:

```sh
hirsel run hirsel.json --grants grants-tree.json -- tree-check proposed.json 4 0
hirsel build hirsel.json --backend llvm -o /tmp/patchctl-tree-llvm
/tmp/patchctl-tree-llvm --hirsel-record /tmp/patchctl-tree.tape --hirsel-grants grants-tree.json tree-apply tree-change.json 4
/tmp/patchctl-tree-llvm --hirsel-replay /tmp/patchctl-tree.tape
```

Native observation/record flags precede `--hirsel-grants`; application operands
follow its template. Keep tapes outside every granted mutable directory. Set
`placement.cores` to 1, 2 or 4 in a private template for production Ordered
launches; this does not change logical jobs. Requested width alone does not
establish effective execution width. No Free scheduling is used.

A tree envelope is JSON with exactly two fields, `schema: 1` and `files`, an
array of records with exactly `path`, `kind`, `source_sha256` and `patch`:

```json
{"schema":1,"files":[{"path":"new.txt","kind":"add","source_sha256":null,"patch":"--- old\n+++ new\n@@ -0,0 +1 @@\n+hello\n"}]}
```

`add` requires absence and a null digest; `modify` and `delete` require an
existing regular file and its exact lowercase SHA-256. An empty modify patch is
identity. A delete patch must produce empty bytes, and deletion means omission
from output. `files: []` copies the source tree. Files absent from the envelope
retain their bytes, including non-UTF-8 bytes. Changed files must be UTF-8.
LF, CRLF and absent final LF are preserved. Source snapshots and all resulting
files are validated before any output write. Per-file conflicts are collected
in path byte order, including every rejected hunk index in original patch order.
For example two stale paths yield two `stale_source` conflict rows, status 1,
and no output writes. Structural/input/I/O/cancellation errors exit 2. A success
prints a JSON summary (`bytes`, `conflicts`, `files`) and exits 0.

Paths must be canonical relative paths of 1..512 bytes with no empty, dot or
parent components, controls, backslashes or colons. `.patchctl-` components are
reserved. Duplicate paths and file/directory collisions are refused before
publication. A patch's header labels have no path authority. There is no rename
or binary-patch inference. Shipped `diff.apply` retains its documented exact
context relocation behavior after digest equality has been established.

Bounds: 32 files; 65536 bytes/file; 524288 source bytes and combined source/patch
bytes; 524288 bytes for the encoded envelope; 256 total hunks; 1048576 output
bytes; jobs 1..4. There is an additional 8192-entry directory traversal bound.
Sizes and path sets are checked before reading file contents; each open read
also uses a bounded read plus an EOF probe. JSON's raw input bound contains DOM
allocation. Std's diff allocations remain bounded by admitted inputs. Large
boundary fixtures can encounter the reference engine's fixed 1M tooling budget;
that is an incomplete engine cell, not a resource refusal by patchctl.

Publication uses `file.put_new` for each validated file. An incomplete marker is
created first; the completion manifest `.patchctl-complete.json` is created
last and lists every result path and SHA-256. Failed publication can leave files
and an incomplete marker, or an output without either marker if the final
completion create fails. Absence of completion means incomplete. This does not
promise tree atomicity or crash durability. Do not edit granted roots during a
run. In particular, a private template must name physically distinct source and
output roots: the shipped Dir API cannot establish host-root identity. The
shipped directory API materializes a sorted directory page before returning it;
patchctl bounds accepted traversal work and file contents, but cannot enforce a
provider page allocation limit. These limits remain explicit incomplete parts
of the strict dogfood qualification; see FRICTION.md.

Modules are explicit in `hirsel.json`: `manifest` owns typed records, generic
outcomes, digests and the envelope codec; `plan` owns safe paths, bounded source
inventory and collisions; `apply` owns bounded task sets and shared immutable
snapshots; `publish` owns the new output tree and completion; `report` owns stable
conflicts; `main` owns the CLI and capability projections; `model` retains the
original single-file policy. The generic `Entry[T]` / `find[T]` seam indexes both
byte snapshots and change records; `Outcome[T]` carries bounded-read bytes and
optional patched bytes (None is a deletion).

The focused `treepatch_29` harness invokes the complete app with public reference
and checked-H simulation adapters and with real file providers on both native
backends. Fixtures independently specify the whole expected tree and retain the
similar missing-newline snapshot. The source tree is compared before/after;
replays remove the original host directories before execution. Engine-specific
incomplete cells and original failures are documented in FRICTION.md.

References: similar `96993d92851eff9a484ea157614de30f80dfdc73`,
`src/udiff.rs:280-330,339-364` and the newline-hint snapshot (Apache-2.0);
Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
`src/path/filepath/path.go:319-346`, `path_test.go:532-562,630-685`
(BSD-3-Clause, Copyright 2009 The Go Authors). The orchestration uses an explicit
bounded worklist and rejects symlinks; similar's rendering is delegated to
shipped std.diff with fixed labels/context=3. No new diff algorithm is included.

similar is Copyright Armin Ronacher and contributors, Apache-2.0; the Apache-2.0
license text and Go BSD-3-Clause license text are retained in
[third-party notices](../../third-party/NOTICES.md). The pinned snapshot's
`a.txt`/`b.txt` labels are changed to `old`/`new`; the newline marker and content
bytes are retained. Go's tree fixture motivates nested files/empty directories;
permission refusal is tested through explicit grants rather than host uid/chmod.
