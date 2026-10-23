# jsonpatchctl

Apply a conditional RFC 6902 patch to a deployment snapshot. The shipped
promotion tests the revision, increases replicas, appends a canary check, copies
a timeout, moves the staging record and updates an escaped label key. An agent
can inspect the result before deciding whether to publish it elsewhere.

From the repository root, with the built `hirsel` and matching std image:

```sh
hirsel check examples/jsonpatchctl/hirsel.json
hirsel run examples/jsonpatchctl/hirsel.json --grants examples/jsonpatchctl/grants.json
hirsel run examples/jsonpatchctl/hirsel.json --grants examples/jsonpatchctl/reject-grants.json
hirsel run examples/jsonpatchctl/hirsel.json --grants '{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":["--help"]}'
```

The rejection exits 2: the second operation tests revision 6 against revision 7,
after the first operation has already edited the working document. Std discards
that working result. Stdout is empty and stderr is exactly:

```json
{"schema":1,"class":"invalid_input","stage":"patch","kind":"TestFailed","operation":1,"path":"/revision","detail":""}
```

Success exits 0 and prints one compact JSON envelope plus LF. Member order is
deterministic and follows the shipped JSON/edit implementation, including
reinsertion of replaced keys. No timestamps, random values or host paths appear.
Root removal prints `{"schema":1,"present":false}`. A present JSON null prints
`{"schema":1,"present":true,"document":null}`. A file must contain a JSON value;
there is no special input spelling for an initially absent document. Root remove
followed by root add can recreate a document in the same patch.

To use your own inputs, copy a grants template, set its directory's `host` to
your input directory and its `args` to `["document.json","patch.json"]`. Relative
host paths resolve from the grants file's directory; see the
[shared grants-base recipe](../../docs/native-verification.md#resolve-directory-hosts-beside-the-grants-file)
for launch refusal context and an isolated bad/correct pair. Filenames must be normalized
relative paths, at most 256 UTF-8 bytes; empty, `.`, `..`, empty path components,
backslashes, colons and NUL are rejected. The tool requests only read access to
the directory and stdout/stderr. Help needs only stdout; rejection needs stderr.
Missing directory authority produces a typed `DirectoryGrant` rejection. A
missing output grant or an output error falls back to the existing launcher
error report and status 1. Launch/configuration errors use the CLI's status 3.

The same package builds with both native backends. For an in-tree compiler,
`python3 tools/buck2/prepare.py` installs the pinned toolchains. Build
`//:hirsel-cli-bin-hirsel`, `//:hirsel-std-image`, `//:hirsel-rt-archives` and
`//tools/buck2/native-toolchain:kit` with `.buck2/bin/buck2 build ... --show-output`.
Put the emitted CLI on PATH as `hirsel`, set `HIRSEL_IMAGE` to the absolute image
path, `HIRSEL_RUNTIME_ARCHIVES` and `HIRSEL_LINK_KIT` to the absolute emitted
archive/kit directories, and `LLVM_SYS_231_PREFIX` to the absolute
`tools/buck2/toolchains/llvm-files` directory. The focused test launcher supplies
these inputs automatically.


```sh
jsonpatch_tmp=$(mktemp -d)
hirsel build examples/jsonpatchctl/hirsel.json --backend llvm -o "$jsonpatch_tmp/llvm"
hirsel build examples/jsonpatchctl/hirsel.json --backend cranelift -o "$jsonpatch_tmp/cranelift"
"$jsonpatch_tmp/llvm" --hirsel-grants examples/jsonpatchctl/grants.json deployment.json promote.patch.json
"$jsonpatch_tmp/cranelift" --hirsel-grants examples/jsonpatchctl/grants.json deployment.json stale.patch.json
rm -r "$jsonpatch_tmp"
```

Each input is limited to 65,536 encoded UTF-8 bytes. `file.read_all_bounded`
rejects excess before accumulating it, with a one-byte EOF probe at the limit.
There are exactly two input operands; argument count is checked before any list
is built. The model repeats the byte checks for direct pure callers. Parsing and
application use explicit inclusive limits: 64 operations, DOM depth 32 (root
zero), 32 pointer tokens, 65,536 DOM payload bytes and 10,000,000 work units per
std call. The patch DOM includes operation wrappers. The byte bound applies
before DOM parsing; the operation bound applies before building the typed patch
list. The JSON parser also has its documented nesting limit. There is no
unbounded record collection or user-selectable cap increase. Output is bounded
by the admitted DOM and JSON escaping.

`model.hir` performs pure validation, transformation and encoding. `main.hir`
owns arguments, grants, bounded reads and output. Parser copies preserve the
borrowed text snapshots; `json_patch.apply` borrows both the parsed document
and patch. The complete envelope is encoded before the first stdout write.
Files are never modified. This is transactional publication with respect to
patch/encoding failure; an OS output error can still leave a short write.

All invalid inputs exit 2 with one JSON record on stderr and empty stdout.
`stage`, `kind`, nullable `operation`, nullable `path` and `detail` distinguish
argument/file/UTF-8/JSON/patch failures. Patch failures preserve the zero-based
operation index and original encoded pointer; missing copy/move sources identify
`from`. Whole-patch validation uses std's index 0 and empty pointer convention.
Nested JSON errors retain their category and byte offset in `detail`.

The focused acceptance test is:

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=jsonpatchctl_23
```

The Rust harness embeds both sources, manifest, fixtures, Python verifier and
`tools/capped` as declared inputs. It checks source formatting/checking,
reachable TIR export, the pure source test, actual interpreter runs and actual
LLVM/Cranelift builds and executions. Fixed independent expectations cover
add/remove parsing, test/add, escaped keys, append, copy, move index shifts,
late failure, missing targets/sources, absence/null, limits, malformed and
duplicate JSON, UTF-8, grants, help and deterministic bytes. Every runtime
process runs serially under `tools/capped` with 60 seconds and 2 GiB. Native
compilation is setup with its own normal build timeout. File contents and
directory entries are compared after every cell; private fixtures, launch file
and the whole temp directory are removed and cleanup is asserted. Boundary
controls generate 65 operations and 65,537 bytes solely to test rejection.
The checked-heap test also verifies retained input aliases and zero live heap
bytes after the pure entry completes. See [FRICTION.md](FRICTION.md) for attempts
and verification results.

Usage structure and vectors are ported from `json-patch` revision
`1b0f707f9f4aad1009f0364f6ca73c5e88cdd85c`, `src/lib.rs:340–385,440–490`
and `tests/basic.rs:7–48` (MIT, Copyright 2017 Ivan Dubrov;
[full license](../../docs/json-merge-patch-LICENSE-MIT.txt)). The shipped std
implements the algorithm; this application adds no patch engine. Deviations:
an owned functional result replaces upstream's in-place undo stack; root remove
returns absence instead of upstream's rejection; std enforces finite limits,
exact decimal equality and strict array append rules. The CLI adds an envelope,
input bounds and capability-based I/O. Additional vector answers are small
hand-derived RFC examples. Object equality is checked independent of member
order, with compact encoding and byte determinism asserted separately.

See the [tested JSON, text and time recipes](../../docs/std-api-recipes.md)
for public module names, contextual variants and ownership at each call.
