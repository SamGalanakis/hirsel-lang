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
